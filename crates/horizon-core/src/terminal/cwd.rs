//! The working directory of the process of a panel.
//!
//! Linux answers from `/proc`, with one `readlink` and no program. macOS has no
//! such file, so two programs of the system answer: `pgrep -P` walks from the
//! process of the panel to its deepest child (a transcript wrapper runs the
//! shell, and the `cd` of the person happens in the shell), and `lsof` reads the
//! directory of that child.
//!
//! No frame asks either system. The answer is a value that the coordination
//! worker prepares for a later frame, and each program is ended by its own
//! process id when [`PROGRAM_BOUND`] passes, so a program that hangs holds no
//! worker for ever. [`CwdReader`] is a value and not a condition of the build,
//! so a test reads the programs of macOS with stand-ins on a private `PATH`, on
//! every system.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use crate::bounded_program::{ProgramEnvironment, run};

/// The longest time that one program of this read may run.
pub const PROGRAM_BOUND: Duration = Duration::from_secs(2);

/// How many children the walk of `pgrep` follows. The tree of a panel holds a
/// transcript wrapper, a shell and the program that the shell runs, so a walk
/// deeper than this is not the tree of a panel. The bound also limits how many
/// programs one read starts.
const MAX_DEPTH: usize = 8;

/// The program that names the child of a process.
const PGREP: &str = "pgrep";

/// The program that names the directory of a process.
const LSOF: &str = "lsof";

/// The system that answers the working directory of a process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CwdReader {
    /// Linux reads `/proc/<pid>/cwd`. No program runs and no bound applies.
    ProcFs,
    /// macOS runs `pgrep` for each level of the tree and then `lsof`.
    Programs,
}

impl CwdReader {
    /// The reader of the system that runs this build.
    #[must_use]
    pub const fn of_this_system() -> Self {
        if cfg!(target_os = "macos") {
            Self::Programs
        } else {
            Self::ProcFs
        }
    }
}

/// The working directory of `pid`, as the system of `reader` reports it.
///
/// Each program that `reader` runs is ended by its own process id when `bound`
/// passes. `None` means no answer: no `/proc` entry, no program of that name, a
/// program that failed, or a bound that passed.
#[must_use]
pub fn read_cwd(reader: CwdReader, pid: u32, programs: &ProgramEnvironment, bound: Duration) -> Option<PathBuf> {
    crate::ui_thread::blocking("the working directory of a panel is read");
    match reader {
        CwdReader::ProcFs => via_procfs(pid),
        CwdReader::Programs => via_programs(pid, programs, bound),
    }
}

fn via_procfs(pid: u32) -> Option<PathBuf> {
    let target_pid = shell_pid_behind_wrapper(pid);
    proc_cwd(target_pid).or_else(|_| proc_cwd(pid)).ok()
}

fn proc_cwd(pid: u32) -> std::io::Result<PathBuf> {
    std::fs::read_link(std::path::Path::new("/proc").join(pid.to_string()).join("cwd"))
}

/// The transcript wrapper (`script`) is the PTY's direct child but never changes
/// its own working directory, so reading its cwd would always report the panel's
/// original spawn directory. When `pid` is that wrapper, return the shell it
/// launched so callers read the shell's live cwd (which tracks `cd`). Returns
/// `pid` unchanged when it is not a `script` wrapper (transcript capture
/// disabled), in which case the PTY child already is the shell.
fn shell_pid_behind_wrapper(pid: u32) -> u32 {
    if proc_comm(pid).as_deref() != Some("script") {
        return pid;
    }
    first_child_pid(pid).unwrap_or(pid)
}

fn proc_comm(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(std::path::Path::new("/proc").join(pid.to_string()).join("comm")).ok()?;
    Some(comm.trim_end().to_string())
}

fn first_child_pid(pid: u32) -> Option<u32> {
    let children = std::fs::read_to_string(
        std::path::Path::new("/proc")
            .join(pid.to_string())
            .join("task")
            .join(pid.to_string())
            .join("children"),
    )
    .ok()?;
    parse_first_child_pid(&children)
}

/// Parse the space-separated PID list from `/proc/<pid>/task/<pid>/children`,
/// returning the first child (the shell launched by the wrapper).
fn parse_first_child_pid(children: &str) -> Option<u32> {
    children.split_whitespace().find_map(|token| token.parse().ok())
}

fn via_programs(pid: u32, programs: &ProgramEnvironment, bound: Duration) -> Option<PathBuf> {
    let target_pid = deepest_child_pid(pid, programs, bound).unwrap_or(pid);
    lsof_cwd_for_pid(target_pid, programs, bound).or_else(|| lsof_cwd_for_pid(pid, programs, bound))
}

fn deepest_child_pid(mut pid: u32, programs: &ProgramEnvironment, bound: Duration) -> Option<u32> {
    let mut saw_child = false;

    for _ in 0..MAX_DEPTH {
        let Some(child_pid) = direct_child_pid(pid, programs, bound) else {
            break;
        };
        saw_child = true;
        pid = child_pid;
    }

    saw_child.then_some(pid)
}

fn lsof_cwd_for_pid(pid: u32, programs: &ProgramEnvironment, bound: Duration) -> Option<PathBuf> {
    let program = programs.find(LSOF)?;
    let pid = pid.to_string();
    let args = [
        OsString::from("-a"),
        OsString::from("-d"),
        OsString::from("cwd"),
        OsString::from("-p"),
        OsString::from(pid),
        OsString::from("-Fn"),
    ];
    let output = run(&program, &args, programs, bound).ok()?;
    if !output.status.success() {
        return None;
    }

    parse_lsof_cwd(std::str::from_utf8(&output.stdout).ok()?)
}

fn parse_lsof_cwd(output: &str) -> Option<PathBuf> {
    let mut in_cwd_entry = false;

    for line in output.lines() {
        if let Some(descriptor) = line.strip_prefix('f') {
            in_cwd_entry = descriptor == "cwd";
            continue;
        }
        if in_cwd_entry && let Some(path) = line.strip_prefix('n') {
            return Some(PathBuf::from(path));
        }
    }

    None
}

fn direct_child_pid(pid: u32, programs: &ProgramEnvironment, bound: Duration) -> Option<u32> {
    let program = programs.find(PGREP)?;
    let args = [OsString::from("-P"), OsString::from(pid.to_string())];
    let output = run(&program, &args, programs, bound).ok()?;
    if !output.status.success() {
        return None;
    }

    parse_pgrep_pid(std::str::from_utf8(&output.stdout).ok()?)
}

fn parse_pgrep_pid(output: &str) -> Option<u32> {
    output.lines().find_map(|line| line.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::{CwdReader, parse_first_child_pid, parse_lsof_cwd, parse_pgrep_pid, read_cwd};
    use crate::bounded_program::ProgramEnvironment;
    use crate::bounded_program::bound_clock::{self, TestClock};
    use crate::util::stand_in_program_in;

    /// A folder of stand-in programs that a test points the reader at, so no
    /// program of the person answers for the panel.
    struct StandIns {
        folder: tempfile::TempDir,
    }

    impl StandIns {
        fn new() -> Self {
            Self {
                folder: tempfile::tempdir().expect("folder of stand-in programs"),
            }
        }

        fn program(&self, name: &str, body: &str) {
            stand_in_program_in(self.folder.path(), name, body).expect("the stand-in program of the test");
        }

        fn environment(&self) -> ProgramEnvironment {
            ProgramEnvironment::with_path(self.folder.path().as_os_str().to_os_string())
        }
    }

    /// Whether the process id still names a process.
    fn process_is_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn the_procfs_reader_reads_the_directory_of_this_process() {
        let cwd = read_cwd(
            CwdReader::ProcFs,
            std::process::id(),
            &ProgramEnvironment::with_path(OsString::new()),
            Duration::from_secs(0),
        )
        .expect("cwd");

        assert_eq!(cwd, std::env::current_dir().expect("current dir"));
    }

    #[test]
    fn the_programs_of_this_system_read_the_deepest_child_of_a_panel() {
        // The panel runs a transcript wrapper, the wrapper runs the shell, and
        // the shell runs the program: only the deepest child knows the
        // directory that the person changed to.
        let stand_ins = StandIns::new();
        let directory = stand_ins.folder.path().join("project");
        std::fs::create_dir_all(&directory).expect("directory of the panel");
        let log = stand_ins.folder.path().join("calls");
        let log_text = log.display().to_string();
        stand_ins.program(
            "pgrep",
            &format!(
                r#"printf '%s\n' "$*" >> "{log_text}"
case "$2" in
100) printf '200\n' ;;
200) printf '300\n' ;;
*) exit 1 ;;
esac"#
            ),
        );
        stand_ins.program(
            "lsof",
            &format!(
                r#"printf '%s\n' "$*" >> "{log_text}"
case "$*" in
*" 200"*) exit 1 ;;
esac
printf 'p300\nfcwd\nn{0}\n'"#,
                directory.display()
            ),
        );

        let cwd = read_cwd(
            CwdReader::Programs,
            100,
            &stand_ins.environment(),
            Duration::from_secs(2),
        );

        assert_eq!(cwd, Some(directory.clone()), "the walk ends at the deepest child");
        let calls = std::fs::read_to_string(&log).expect("the stand-ins recorded their calls");
        assert!(calls.contains("-P 100"), "the walk starts at the panel: {calls}");
        assert!(calls.contains("-p 300 -Fn"), "lsof reads the deepest child: {calls}");
    }

    /// The pid that a stand-in wrote, once it has written the whole of it.
    fn pid_in(file: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(text) = std::fs::read_to_string(file)
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "the stand-in of this test never wrote its pid"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_program_that_passes_its_bound_is_ended_by_its_pid() {
        let stand_ins = StandIns::new();
        stand_ins.program("pgrep", "exit 1");
        let pid_file = stand_ins.folder.path().join("lsof-pid");
        // `exec` keeps the process id of the script, so the program that hangs is
        // the child that the bound ends and no second program waits beside it.
        stand_ins.program(
            "lsof",
            &format!(
                r#"printf '%s\n' "$$" > "{0}"
exec /bin/sleep 60"#,
                pid_file.display()
            ),
        );

        // The bound of this read runs on a clock that this test moves, so the bound
        // begins to count only after the stand-in has written its pid: a machine that
        // starts `/bin/sh` more slowly than the bound allows cannot take the bound
        // away from the program that the bound must end.
        let clock = Arc::new(TestClock::new());
        let environment = stand_ins.environment();
        let (answered, answer) = std::sync::mpsc::channel();
        let read = {
            let clock = Arc::clone(&clock);
            std::thread::spawn(move || {
                let _installed = bound_clock::install(clock);
                let cwd = read_cwd(
                    CwdReader::Programs,
                    std::process::id(),
                    &environment,
                    Duration::from_millis(300),
                );
                let _ = answered.send(cwd);
            })
        };
        let pid = pid_in(&pid_file);
        // The walk starts one program for each level of the tree and reads the
        // directory with another one, so the test moves the clock past the bound of
        // every program of the read until the read answers.
        let deadline = Instant::now() + Duration::from_secs(20);
        let cwd = loop {
            clock.advance(Duration::from_millis(300));
            match answer.recv_timeout(Duration::from_millis(50)) {
                Ok(cwd) => break cwd,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => assert!(
                    Instant::now() < deadline,
                    "the read of this test did not answer at its bound"
                ),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("the read of this test ended without an answer")
                }
            }
        };
        read.join().expect("the read of this test");

        assert_eq!(cwd, None, "a bound that passes answers no directory");
        assert!(
            !process_is_alive(pid),
            "the bound kills the exact child that it started: {pid}"
        );
    }

    #[test]
    fn a_read_ends_a_program_that_passes_its_bound_without_waiting_for_it() {
        let stand_ins = StandIns::new();
        stand_ins.program("pgrep", "exit 1");
        stand_ins.program("lsof", "exec /bin/sleep 60");

        // The clock of this process: the bound of the 300 ms read ends the program
        // and the caller waits that bound, and not the program.
        let started = Instant::now();
        let cwd = read_cwd(
            CwdReader::Programs,
            std::process::id(),
            &stand_ins.environment(),
            Duration::from_millis(300),
        );
        let waited = started.elapsed();

        assert_eq!(cwd, None, "a bound that passes answers no directory");
        assert!(
            waited >= Duration::from_millis(300),
            "the read waits for its bound: {waited:?}"
        );
        assert!(
            waited < Duration::from_secs(5),
            "the bound ends the read instead of waiting for the program: {waited:?}"
        );
    }

    #[test]
    fn a_reader_whose_programs_are_missing_answers_nothing() {
        let cwd = read_cwd(
            CwdReader::Programs,
            std::process::id(),
            &ProgramEnvironment::with_path(OsString::new()),
            Duration::from_secs(2),
        );

        assert_eq!(cwd, None);
    }

    #[test]
    fn parse_lsof_cwd_extracts_working_directory() {
        let output = "p1234\nfcwd\nn/tmp/project\n";

        assert_eq!(parse_lsof_cwd(output), Some(PathBuf::from("/tmp/project")));
    }

    #[test]
    fn parse_lsof_cwd_returns_none_without_cwd_entry() {
        let output = "p1234\nf1\nn/tmp/project/file.txt\n";

        assert_eq!(parse_lsof_cwd(output), None);
    }

    #[test]
    fn parse_pgrep_pid_extracts_first_child_pid() {
        let output = "12027\n12099\n";

        assert_eq!(parse_pgrep_pid(output), Some(12027));
    }

    #[test]
    fn parse_first_child_pid_extracts_first_child() {
        assert_eq!(parse_first_child_pid("31219 31300\n"), Some(31219));
    }

    #[test]
    fn parse_first_child_pid_returns_none_when_empty() {
        assert_eq!(parse_first_child_pid(""), None);
        assert_eq!(parse_first_child_pid("\n"), None);
    }

    #[test]
    fn the_guarded_read_names_a_directory_of_a_panel() {
        // The read starts programs on macOS and reads a file on Linux, so it is
        // never a call of a frame: the guard accepts it only on a worker.
        let stand_ins = StandIns::new();
        stand_ins.program("pgrep", "exit 1");
        stand_ins.program("lsof", "exit 1");
        let frame = crate::ui_thread::FrameScope::enter();
        let message = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::ui_thread::use_strict_mode();
            read_cwd(
                CwdReader::Programs,
                std::process::id(),
                &stand_ins.environment(),
                Duration::from_millis(100),
            )
        }));
        drop(frame);

        let panic = message.expect_err("a frame that reads the directory reports the wait");
        let text = panic
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_else(|| "<no message>".to_owned());
        assert!(text.contains("working directory of a panel"), "{text}");
        assert!(text.contains("cwd.rs"), "the message names this module: {text}");
    }
}
