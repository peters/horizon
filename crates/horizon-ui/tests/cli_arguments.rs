//! An argument of this build is judged before the start touches the home, the
//! settings, the sessions or the skills of the person.
//!
//! This is the only test of the workspace that starts the binary of the UI
//! crate, and it gives every start its own home in a temporary folder and no
//! display. A start that reaches the window would hang, so the helper stops the
//! exact child after ten seconds and the test fails.
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(10);

/// Starts the binary of this crate with `arguments`.
fn start(arguments: &[&str]) -> Outcome {
    start_binary(Path::new(env!("CARGO_BIN_EXE_horizon")), arguments)
}

/// The first line of the usage, which `--help` writes and every refusal repeats.
const USAGE_FIRST_LINE: &str = "usage: horizon [<folder>] [--config <path>] [--ephemeral] [--new-session] [--blank]";

/// One start of the binary and what it left in its private home.
struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
    entries_in_home: usize,
}

impl Outcome {
    /// The first line of the standard output, without the line end.
    fn first_stdout_line(&self) -> &str {
        self.stdout.lines().next().unwrap_or_default()
    }

    /// The first line of the standard error, without the line end.
    fn first_stderr_line(&self) -> &str {
        self.stderr.lines().next().unwrap_or_default()
    }
}

/// Starts `binary` with `arguments`, a private home and no display.
fn start_binary(binary: &Path, arguments: &[&str]) -> Outcome {
    let home = tempfile::tempdir().expect("the private home of the start");
    start_with_home(binary, home.path(), None, arguments)
}

/// Starts `binary` with `arguments`, the given home of the person, an optional
/// `HORIZON_HOME`, and no display.
fn start_with_home(binary: &Path, home: &Path, horizon_home: Option<&Path>, arguments: &[&str]) -> Outcome {
    start_with_integrations(binary, home, horizon_home, arguments, false)
}

/// Starts `binary` as a person starts it, with the agent integrations of the home
/// of that person on, so a test sees every file that a start of this home writes.
fn start_like_a_person(binary: &Path, home: &Path, horizon_home: Option<&Path>, arguments: &[&str]) -> Outcome {
    start_with_integrations(binary, home, horizon_home, arguments, true)
}

/// Starts `binary` with `arguments`, the given home of the person, an optional
/// `HORIZON_HOME`, no display, and the agent integrations that `integrations` says.
fn start_with_integrations(
    binary: &Path,
    home: &Path,
    horizon_home: Option<&Path>,
    arguments: &[&str],
    integrations: bool,
) -> Outcome {
    let output = tempfile::tempdir().expect("the folder of the output");
    let written = |name: &str| {
        std::fs::File::create(output.path().join(name)).unwrap_or_else(|error| panic!("create {name}: {error}"))
    };
    // A copy of this build is a program that another thread may still hold open for
    // writing, and the kernel refuses to execute such a file: the helper builds the
    // command line again and starts it again, as every test that starts one does.
    let mut child = horizon_core::spawn_stand_in(|| {
        let mut command = Command::new(binary);
        command.args(arguments).env_clear().env("HOME", home);
        if !integrations {
            command.env("HORIZON_AGENT_INTEGRATIONS", "off");
        }
        command.env_remove("DISPLAY");
        if let Some(horizon_home) = horizon_home {
            command.env("HORIZON_HOME", horizon_home);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(written("stdout")))
            .stderr(Stdio::from(written("stderr")))
            .spawn()
    })
    .unwrap_or_else(|error| panic!("start `{}`: {error}", binary.display()));
    let deadline = Instant::now() + BOUND;
    let status = loop {
        if let Some(status) = child.try_wait().expect("ask the start") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "`horizon {}` still ran after {BOUND:?}: it reached the window instead of judging the argument",
                arguments.join(" ")
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let read = |name: &str| {
        std::fs::read_to_string(output.path().join(name)).unwrap_or_else(|error| panic!("read {name}: {error}"))
    };
    Outcome {
        code: status.code().expect("the exit code of the start"),
        stdout: read("stdout"),
        stderr: read("stderr"),
        entries_in_home: std::fs::read_dir(home).expect("read the private home").count(),
    }
}

/// The usage and the version end the start before the home is prepared.
#[test]
fn the_usage_and_the_version_leave_the_home_alone() {
    let usage = USAGE_FIRST_LINE;

    for arguments in [&["--help"][..], &["-h"]] {
        let outcome = start(arguments);
        assert_eq!(outcome.code, 0, "`{arguments:?}`: {:?}", outcome.stderr);
        assert_eq!(outcome.first_stdout_line(), usage, "`{arguments:?}`");
        assert_eq!(outcome.entries_in_home, 0, "`{arguments:?}` prepared the home");
    }

    let expected = format!("horizon {}", env!("CARGO_PKG_VERSION"));
    for arguments in [&["--version"][..], &["-V"]] {
        let outcome = start(arguments);
        assert_eq!(outcome.code, 0, "`{arguments:?}`: {:?}", outcome.stderr);
        assert_eq!(outcome.first_stdout_line(), expected, "`{arguments:?}`");
        assert_eq!(outcome.entries_in_home, 0, "`{arguments:?}` prepared the home");
    }
}

/// An argument that this build does not know names itself and ends the start,
/// which is what a start by mistake of a test runner or of `--help` must do.
#[test]
fn an_argument_that_this_build_does_not_know_ends_the_start() {
    for (arguments, named) in [
        (&["--list"][..], "--list"),
        (&["--nocapture"], "--nocapture"),
        (&["--definitely-unknown"], "--definitely-unknown"),
    ] {
        let outcome = start(arguments);
        assert_eq!(outcome.code, 2, "`{arguments:?}`: {:?}", outcome.stdout);
        assert_eq!(
            outcome.first_stderr_line(),
            format!("error: unknown argument {named}"),
            "`{arguments:?}`"
        );
        assert_eq!(outcome.entries_in_home, 0, "`{arguments:?}` prepared the home");
    }

    // A known mode refuses an argument that is not part of it, so an entry in a
    // client cannot fall through to another mode either.
    let browser_mcp = start(&["--browser-mcp", "--list"]);
    assert_eq!(browser_mcp.code, 2);
    assert_eq!(browser_mcp.entries_in_home, 0);
    let server = start(&["--server", "stray"]);
    assert_eq!(server.code, 2);
    assert_eq!(server.entries_in_home, 0);
}

/// A folder that a start names is judged before the home is prepared, and its
/// refusal is one sentence that names the path.
#[test]
fn a_folder_that_is_not_there_ends_the_start_with_one_sentence() {
    let outcome = start(&["stray"]);

    assert_eq!(outcome.code, 2, "{:?}", outcome.stdout);
    assert_eq!(outcome.first_stderr_line(), "error: stray is not there");
    assert_eq!(outcome.stderr.lines().count(), 1, "one sentence, not the usage");
    assert_eq!(outcome.entries_in_home, 0, "the start prepared the home");

    let file = tempfile::tempdir().expect("the folder of a file");
    let path = file.path().join("a-file");
    std::fs::write(&path, b"not a folder").expect("the file");
    let outcome = start(&[path.to_str().expect("a path of text")]);
    assert_eq!(outcome.code, 2);
    assert_eq!(
        outcome.first_stderr_line(),
        format!("error: {} is not a folder", path.display())
    );
    assert_eq!(outcome.entries_in_home, 0);
}

/// The usage follows the error line of a refusal, which is the line that a
/// person reads in a terminal.
#[test]
fn a_refusal_writes_the_usage_below_its_error() {
    let outcome = start(&["--list"]);
    let lines: Vec<&str> = outcome.stderr.lines().collect();
    assert_eq!(lines.first().copied(), Some("error: unknown argument --list"));
    assert_eq!(lines.get(1).copied(), Some(USAGE_FIRST_LINE));
}

/// The copy of the binary that cargo keeps in `deps` is refused before the home
/// is prepared, while a mode that ends early still works from it.
#[test]
fn the_application_refuses_to_start_from_a_build_folder() {
    let folder = tempfile::tempdir().expect("the folder of the copy");
    let deps = folder.path().join("deps");
    std::fs::create_dir(&deps).expect("the folder that cargo keeps the binaries in");
    let copied = deps.join("horizon");
    std::fs::copy(env!("CARGO_BIN_EXE_horizon"), &copied).expect("copy the binary");

    let refused = start_binary(&copied, &[]);
    assert_eq!(refused.code, 2, "the copy started the window: {:?}", refused.stderr);
    assert_eq!(
        refused.first_stderr_line(),
        "error: this file is a build artifact; start target/<profile>/horizon"
    );
    assert_eq!(refused.entries_in_home, 0, "the copy prepared the home");

    let usage = start_binary(&copied, &["--help"]);
    assert_eq!(usage.code, 0, "{:?}", usage.stderr);
    assert_eq!(usage.first_stdout_line(), USAGE_FIRST_LINE);
    assert_eq!(usage.entries_in_home, 0);

    let unknown = start_binary(&copied, &["--list"]);
    assert_eq!(unknown.code, 2);
    assert_eq!(unknown.first_stderr_line(), "error: unknown argument --list");
}

/// The requests of the folder channel of a home.
fn request_folder(horizon_home: &Path) -> std::path::PathBuf {
    horizon_core::folder_requests::directory(horizon_home)
}

/// Starts a window of this home that takes the request of a start and answers it
/// with `answer`.
fn answer_as_the_window(
    horizon_home: &Path,
    answer: horizon_core::folder_requests::Answer,
) -> std::thread::JoinHandle<()> {
    let root = horizon_home.to_path_buf();
    std::thread::spawn(move || {
        let deadline = Instant::now() + BOUND;
        while Instant::now() < deadline {
            let now = horizon_core::folder_requests::now_millis();
            let claimed = horizon_core::folder_requests::claim(&root, now).unwrap_or_default();
            if let Some(request) = claimed.first() {
                horizon_core::folder_requests::answer(&root, request, &answer)
                    .expect("the answer of the window of this test");
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the start wrote no request into the home of this test");
    })
}

/// One start of this home, with its session and its lease, as a window that runs
/// leaves them, and the store of that window.
///
/// A window that runs holds its store. The writer of a store writes the index of
/// the sessions on its own worker, and a store that is let go closes its files
/// there, so both change the home after the call that asked for them. The store
/// comes back quiet, with every write that it queued on the disk, and the test
/// holds it until its end: the window of the test writes nothing while a second
/// start runs.
fn home_with_a_running_window(temp: &tempfile::TempDir) -> (horizon_core::HorizonHome, horizon_core::SessionStore) {
    use std::os::unix::fs::PermissionsExt as _;

    let root = temp.path().join(".horizon");
    std::fs::create_dir_all(&root).expect("the home");
    // A start of this home refuses a home that other users reach, so the test
    // gives its home the mode that a start gives it.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("a private home");
    let horizon_home = horizon_core::HorizonHome::from_root(root);
    let store = horizon_core::SessionStore::new(horizon_home.clone(), horizon_home.config_path());
    let session = store
        .create_session_from_runtime(horizon_core::RuntimeState::from_config(&horizon_core::Config::default()))
        .expect("a session of this home");
    store.acquire_lease(&session.session_id).expect("the lease of a window");
    assert!(
        store.flush_runtime_states(BOUND),
        "the window of this test wrote its session and the index of the sessions"
    );
    (horizon_home, store)
}

/// A second `horizon <folder>` of a home whose Horizon runs reaches that window
/// instead of printing that the folder was not opened.
#[test]
fn a_folder_of_a_second_start_reaches_the_window_that_runs() {
    let binary = Path::new(env!("CARGO_BIN_EXE_horizon"));
    let temp = tempfile::tempdir().expect("the home of the test");
    let (horizon_home, _window_store) = home_with_a_running_window(&temp);
    let folder = temp.path().join("repo");
    std::fs::create_dir(&folder).expect("the folder");
    let folder = horizon_core::folder_identity(&folder);

    let window = answer_as_the_window(
        horizon_home.root(),
        horizon_core::folder_requests::Answer::Made {
            workspace: "repo".to_string(),
        },
    );
    let outcome = start_with_home(
        binary,
        temp.path(),
        Some(horizon_home.root()),
        &[folder.to_str().expect("a path of text")],
    );
    window.join().expect("the window of this test");

    assert_eq!(outcome.code, 0, "{:?}", outcome.stderr);
    assert_eq!(
        outcome.first_stdout_line(),
        format!("Horizon made a workspace for the folder {}.", folder.display())
    );
    assert!(
        !outcome.stderr.lines().any(|line| line.starts_with("error:")),
        "a request that a window answered is no error: {:?}",
        outcome.stderr
    );
    let left: Vec<std::ffi::OsString> = std::fs::read_dir(request_folder(horizon_home.root()))
        .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "the start removes its files: {left:?}");
}

/// A home whose Horizon runs but answers nothing leaves one sentence and the
/// status of a request that was served, because a busy window is no error.
#[test]
fn a_folder_of_a_second_start_that_no_window_answers_ends_with_one_sentence() {
    let binary = Path::new(env!("CARGO_BIN_EXE_horizon"));
    let temp = tempfile::tempdir().expect("the home of the test");
    let (horizon_home, _window_store) = home_with_a_running_window(&temp);
    let folder = temp.path().join("repo");
    std::fs::create_dir(&folder).expect("the folder");
    let folder = horizon_core::folder_identity(&folder);

    let outcome = start_with_home(
        binary,
        temp.path(),
        Some(horizon_home.root()),
        &[folder.to_str().expect("a path of text")],
    );

    // The caller waits its own bound, which is why this start takes seconds.
    assert_eq!(outcome.code, 0, "{:?}", outcome.stderr);
    assert_eq!(
        outcome.first_stdout_line(),
        format!(
            "Horizon runs for this home, but it did not answer, so the folder {} was not opened there.",
            folder.display()
        )
    );
    assert_eq!(outcome.stdout.lines().count(), 1, "{:?}", outcome.stdout);
    let left: Vec<std::ffi::OsString> = std::fs::read_dir(request_folder(horizon_home.root()))
        .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "the start removes its files: {left:?}");
}

/// Every file under `root`, with its size and the time that it changed, and every
/// folder by its name.
///
/// `skip` is the folder of the requests of a second start: that is the one folder
/// that a start which only asks a window writes.
fn home_listing(root: &Path, skip: &Path) -> Vec<String> {
    fn walk(root: &Path, skip: &Path, folder: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            if path.starts_with(skip) {
                continue;
            }
            if metadata.is_dir() {
                out.push(format!("{} is a folder", relative.display()));
                walk(root, skip, &path, out);
            } else {
                let changed = metadata
                    .modified()
                    .map_or_else(|_| "no time".to_string(), |time| format!("{time:?}"));
                out.push(format!(
                    "{} holds {} bytes, changed {changed}",
                    relative.display(),
                    metadata.len()
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, skip, root, &mut out);
    out.sort();
    out
}

/// A start that names a folder while a window of this home runs is a client of
/// that window: it writes its request, prints one sentence, and changes no file of
/// the home of the person.
#[test]
fn a_folder_of_a_second_start_writes_nothing_but_its_request() {
    let binary = Path::new(env!("CARGO_BIN_EXE_horizon"));
    let temp = tempfile::tempdir().expect("the home of the test");
    let (horizon_home, _window_store) = home_with_a_running_window(&temp);
    let folder = temp.path().join("repo");
    std::fs::create_dir(&folder).expect("the folder");
    let folder = horizon_core::folder_identity(&folder);

    let window = answer_as_the_window(
        horizon_home.root(),
        horizon_core::folder_requests::Answer::Shown {
            workspace: "repo".to_string(),
        },
    );
    let requests = horizon_core::folder_requests::directory(horizon_home.root());
    let requests = requests
        .parent()
        .expect("the folder of the requests of a home")
        .to_path_buf();
    // The window of this test is quiet and the second start has ended when the
    // second picture is taken, so the two pictures need no wait.
    let before = home_listing(temp.path(), &requests);
    let outcome = start_like_a_person(
        binary,
        temp.path(),
        Some(horizon_home.root()),
        &[folder.to_str().expect("a path of text")],
    );
    window.join().expect("the window of this test");

    assert_eq!(outcome.code, 0, "{:?}", outcome.stderr);
    assert_eq!(
        outcome.first_stdout_line(),
        format!("Horizon shows the folder {}.", folder.display())
    );
    assert_eq!(
        outcome.stdout.lines().count(),
        1,
        "a client of a running window prints one sentence: {:?}",
        outcome.stdout
    );
    assert!(
        outcome.stderr.is_empty(),
        "a client of a running window writes no line of the log: {:?}",
        outcome.stderr
    );
    assert_eq!(
        home_listing(temp.path(), &requests),
        before,
        "a start that only asks a window changes no file of the home of the person"
    );
}
