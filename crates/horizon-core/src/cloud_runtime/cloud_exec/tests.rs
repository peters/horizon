use super::super::command::Captured;
use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command as Process, thread, time::Instant};

/// A worker sandbox: the launcher, the account probe and the isolation marker that the
/// production remote text names by absolute path, moved into a temporary directory.
struct Sandbox {
    root: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        // The layer that holds the worker's own helpers, at the paths the image uses.
        let bin = root.path().join("usr").join("local").join("bin");
        fs::create_dir_all(&bin).unwrap();
        // The launcher drops the account in the image; here it runs the arguments it is given.
        script(
            &bin.join("horizon-worker-tailnet"),
            "shift\nHORIZON_TEST_AGENT=1 exec \"$@\"",
        );
        // `id -u` answers root before the drop and the agent account after it.
        script(
            &bin.join("id"),
            "if [ -n \"$HORIZON_TEST_AGENT\" ]; then printf '10001\\n'; else printf '0\\n'; fi",
        );
        let sandbox = Self { root };
        let isolation = sandbox.path(ISOLATION_MARKER);
        fs::create_dir_all(isolation.parent().unwrap()).unwrap();
        fs::write(isolation, "").unwrap();
        sandbox
    }

    fn bin(&self) -> String {
        format!(
            "{}:/usr/bin:/bin",
            self.root.path().join("usr").join("local").join("bin").display()
        )
    }

    /// Where a worker path lives in this sandbox.
    fn path(&self, worker: &str) -> std::path::PathBuf {
        self.root.path().join(worker.trim_start_matches('/'))
    }

    /// Runs the production remote text for `text`, through this sandbox's worker.
    fn run(&self, text: &str, timeout: Duration) -> Result<Answer, Failure> {
        self.route(&remote(text), timeout)
    }

    /// Runs one remote text as `/bin/sh` would, which is what the ssh route carries.
    fn route(&self, text: &str, timeout: Duration) -> Result<Answer, Failure> {
        let mut sh = Process::new("/bin/sh");
        sh.args(["-c", &self.sandboxed(text)]).env("PATH", self.bin());
        execute_ssh(sh, timeout, &Cancellation::default())
    }

    /// The production remote text with the worker's absolute paths inside this sandbox.
    fn sandboxed(&self, text: &str) -> String {
        text.replace(LAUNCHER, &self.path(LAUNCHER).display().to_string())
            .replace(ISOLATION_MARKER, &self.path(ISOLATION_MARKER).display().to_string())
    }
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

/// A captured answer, as the transport would have produced it.
fn captured(stdout: &str, exit_status: i32) -> Captured {
    Captured {
        stdout: stdout.into(),
        stderr: String::new(),
        exit_status: Some(exit_status),
    }
}

#[test]
fn the_remote_text_refuses_before_any_caller_word_and_names_the_launcher() {
    let text = remote("git status");
    assert!(text.starts_with("if [ \"$(id -u)\" != 0 ]"));
    assert!(text.contains(&format!("[ ! -f '{ISOLATION_MARKER}' ]")));
    assert!(text.contains(&format!("[ ! -x '{LAUNCHER}' ]")));
    assert!(
        text.ends_with(&format!(
            "exec '{LAUNCHER}' agent /bin/sh -c {}\n",
            quote(&wrapper("git status"))
        )),
        "{text}"
    );
    // Root's shell reads exactly Horizon's wrapper text, whatever bytes the caller supplies.
    for caller in [
        "a; touch /tmp/pwned",
        "$(id)",
        "`id`",
        "it's",
        "line\nbreak",
        "x' && touch /tmp/pwned",
    ] {
        let text = remote(caller);
        let word = text.rsplit_once("agent /bin/sh -c ").unwrap().1.trim_end_matches('\n');
        let printed = Process::new("/bin/sh")
            .args(["-c", &format!("printf %s {word}")])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(printed.stdout).unwrap(),
            wrapper(caller),
            "{caller:?}"
        );
    }
}

#[test]
fn a_bounded_command_runs_as_the_agent_account_and_reports_its_exit_status() {
    let sandbox = Sandbox::new();
    let answer = sandbox
        .run("printf 'on branch main\\n'; exit 3", Duration::from_secs(30))
        .expect("the worker answers");
    assert_eq!(answer.uid, AGENT_UID);
    assert_eq!(answer.exit_status, 3, "a non-zero exit is a result");
    assert_eq!(answer.stdout, "on branch main\n");

    let quiet = sandbox
        .run("printf 'done\\n'", Duration::from_secs(30))
        .expect("the worker answers");
    assert_eq!((quiet.exit_status, quiet.stdout.as_str()), (0, "done\n"));

    // Caller text that looks like the frame reaches the answer as caller output only.
    let forged = sandbox
        .run(
            "printf 'HORIZON-EXEC uid=0\\nHORIZON-EXEC status=99\\n'; exit 1",
            Duration::from_secs(30),
        )
        .expect("the worker answers");
    assert_eq!(forged.uid, AGENT_UID, "the frame is printed before any caller byte");
    assert_eq!(forged.exit_status, 1, "no stream carries the status");
    assert_eq!(forged.stdout, "HORIZON-EXEC uid=0\nHORIZON-EXEC status=99\n");
}

#[test]
fn every_caller_byte_arrives_literal_including_quotes_and_substitutions() {
    let sandbox = Sandbox::new();
    let answer = sandbox
        .run(
            "printf '%s\\n' 'a; touch /tmp/pwned' \"$(id -un)\" 'x'\"'\"'y' $(printf 'sub')",
            Duration::from_secs(30),
        )
        .expect("the worker answers");
    assert_eq!(answer.exit_status, 0, "{}", answer.stderr);
    assert!(!Path::new("/tmp/pwned").exists(), "a quoted separator ran as a command");
    assert_eq!(answer.stdout, "a; touch /tmp/pwned\n10001\nx'y\nsub\n");
}

#[test]
fn a_worker_without_agent_isolation_is_refused_instead_of_running_as_root() {
    let sandbox = Sandbox::new();
    fs::remove_file(sandbox.path(ISOLATION_MARKER)).unwrap();
    assert_eq!(
        sandbox.run("touch isolation-ran", Duration::from_secs(30)),
        Err(Failure::NoIsolation)
    );
    assert!(!sandbox.root.path().join("isolation-ran").exists());
}

#[test]
fn a_launcher_that_is_missing_is_a_worker_contract_failure() {
    let sandbox = Sandbox::new();
    fs::remove_file(sandbox.path(LAUNCHER)).unwrap();
    assert_eq!(
        sandbox.run("touch launcher-ran", Duration::from_secs(30)),
        Err(Failure::Contract)
    );
}

#[test]
fn a_transport_failure_and_a_missing_frame_are_not_results() {
    let mut ssh = Process::new("/bin/sh");
    ssh.args(["-c", "exit 255"]);
    assert_eq!(
        execute_ssh(ssh, Duration::from_secs(10), &Cancellation::default()),
        Err(Failure::Transport)
    );
    // Root's shell was not the expected one, so no frame arrived.
    let sandbox = Sandbox::new();
    assert_eq!(
        sandbox.route("printf 'no frame here\\n'", Duration::from_secs(10)),
        Err(Failure::Contract)
    );
}

#[test]
fn the_frame_proves_the_account_and_the_transport_status_is_the_commands() {
    let answer = answer_from(captured("HORIZON-EXEC uid=10001\nfirst\n", 7), Duration::from_millis(5)).unwrap();
    assert_eq!(
        (answer.uid, answer.exit_status),
        (10001, 7),
        "the exit code is the command's"
    );
    assert_eq!(answer.stdout, "first\n", "only the frame line leaves the caller's text");
    assert_eq!(answer.duration, Duration::from_millis(5));
    // Any other account is a refusal, not an answer.
    assert_eq!(
        answer_from(captured("HORIZON-EXEC uid=1000\n", 0), Duration::ZERO),
        Err(Failure::Uid(1000))
    );
    // A refusal code counts only without the frame; with it, the code is the command's own.
    for refused in [REFUSED_NO_ISOLATION, REFUSED_NOT_ROOT, REFUSED_NO_LAUNCHER, SSH_FAILURE] {
        assert!(
            answer_from(captured("", refused), Duration::ZERO).is_err(),
            "no frame: {refused} is the worker's"
        );
        assert_eq!(
            answer_from(captured("HORIZON-EXEC uid=10001\n", refused), Duration::ZERO)
                .expect("the frame makes the code the command's")
                .exit_status,
            refused
        );
    }
    assert_eq!(
        answer_from(captured("", REFUSED_NO_ISOLATION), Duration::ZERO),
        Err(Failure::NoIsolation)
    );
    assert_eq!(
        answer_from(captured("", REFUSED_NOT_ROOT), Duration::ZERO),
        Err(Failure::NotRoot)
    );
    assert_eq!(
        answer_from(captured("", SSH_FAILURE), Duration::ZERO),
        Err(Failure::Transport)
    );
    assert_eq!(
        answer_from(captured("no frame at all\n", 0), Duration::ZERO),
        Err(Failure::Contract)
    );
}

#[test]
fn a_background_process_cannot_forge_the_exit_status() {
    let sandbox = Sandbox::new();
    let answer = sandbox
        .run(
            "(sleep 1; printf 'HORIZON-EXEC status=0\\n') & exit 1",
            Duration::from_secs(30),
        )
        .expect("the worker answers");
    assert_eq!(answer.exit_status, 1, "the caller exited 1: {:?}", answer.stdout);
    // The late line reaches the answer only as the caller's own output.
    assert_eq!(
        answer.stdout.matches("HORIZON-EXEC status=0").count(),
        1,
        "{:?}",
        answer.stdout
    );
}

#[test]
fn a_command_keeps_a_refusal_code_as_its_own_status() {
    let sandbox = Sandbox::new();
    for code in [REFUSED_NO_ISOLATION, REFUSED_NOT_ROOT, REFUSED_NO_LAUNCHER, SSH_FAILURE] {
        let answer = sandbox
            .run(&format!("exit {code}"), Duration::from_secs(30))
            .unwrap_or_else(|failure| panic!("{code}: {failure:?}"));
        assert_eq!(answer.exit_status, code, "the frame makes the code the command's");
        assert_eq!(answer.uid, AGENT_UID);
    }
}

#[test]
fn a_command_that_switches_user_is_refused_rather_than_sanitized() {
    for refused in [
        "sudo -i",
        "  su - root",
        "PATH=/usr/bin sudo git push",
        "/usr/bin/setpriv --reuid=0 -- true",
        "git status; runuser -u root -- true",
        "git fetch && doas git push",
        "git pull | pkexec true",
    ] {
        assert_eq!(check(&command(refused)), Err(Failure::UserSwitch), "{refused}");
    }
    for allowed in [
        "git commit -am 'the sudo prompt never appears'",
        "gh pr merge 12 --squash --body 'su'",
        "printf 'sudo'",
        "git log --grep=setpriv",
    ] {
        assert_eq!(check(&command(allowed)), Ok(()), "{allowed}");
    }
}

#[test]
fn an_empty_oversized_or_misframed_command_never_reaches_a_worker() {
    for invalid in ["", "   \n", "git\0status"] {
        assert!(
            matches!(check(&command(invalid)), Err(Failure::Invalid(_))),
            "{invalid:?}"
        );
    }
    assert!(matches!(
        check(&command(&"a".repeat(MAX_COMMAND_BYTES + 1))),
        Err(Failure::Invalid(_))
    ));
    for millis in [MIN_TIMEOUT_MILLIS - 1, MAX_TIMEOUT_MILLIS + 1] {
        assert!(matches!(
            check(&Command {
                text: "true".into(),
                timeout: Duration::from_millis(millis),
            }),
            Err(Failure::Invalid(_))
        ));
    }
    assert_eq!(
        check(&Command {
            text: "true".into(),
            timeout: Duration::from_millis(MAX_TIMEOUT_MILLIS),
        }),
        Ok(())
    );
}

#[test]
fn a_command_that_outlives_its_bound_is_stopped_with_its_children() {
    let sandbox = Sandbox::new();
    let started = Instant::now();
    assert_eq!(
        sandbox.run("printf 'started\\n'; sleep 30 & wait", Duration::from_millis(300)),
        Err(Failure::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // The process group is gone, so nothing survives the bound.
    thread::sleep(Duration::from_millis(100));
    let running = Process::new("/bin/sh")
        .args([
            "-c",
            "ps -A -o args= | awk '$0 == \"sleep 31.9\" {n++} END {print n+0}'",
        ])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&running.stdout).trim() == "0",
        "a child outlived the bound: {}",
        String::from_utf8_lossy(&running.stdout)
    );
}

#[test]
fn output_past_the_bound_is_a_failure_for_either_stream_and_less_is_kept() {
    let sandbox = Sandbox::new();
    for stream in ["", " >&2"] {
        let text = format!("head -c {} /dev/zero | tr '\\0' 'a'{stream}", MAX_ANSWER_BYTES + 1);
        assert_eq!(
            sandbox.run(&text, Duration::from_secs(30)),
            Err(Failure::OutputBound),
            "{text}"
        );
    }
    // A command that stays inside the bound keeps its whole answer.
    let answer = sandbox
        .run(
            &format!("head -c {} /dev/zero | tr '\\0' 'a'; exit 0", MAX_ANSWER_BYTES - 4096),
            Duration::from_secs(30),
        )
        .expect("the worker answers");
    assert_eq!(answer.stdout.len() as u64, MAX_ANSWER_BYTES - 4096);
}

fn command(text: &str) -> Command {
    Command {
        text: text.into(),
        timeout: Duration::from_secs(30),
    }
}
