use super::super::cloud_exec::{self, ISOLATION_MARKER};
use super::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command as Process,
    time::Instant,
};

/// A worker sandbox: a fake `tmux` on the worker's socket, the launcher, the account probe
/// and the isolation marker that the production remote text names. The fake tmux keeps its
/// sessions, buffers and panes in files, so the test reads what a real terminal would show.
struct Sandbox {
    root: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("usr/local/bin");
        fs::create_dir_all(&bin).unwrap();
        script(
            &bin.join("horizon-worker-tailnet"),
            "shift\nHORIZON_TEST_AGENT=1 exec \"$@\"",
        );
        script(
            &bin.join("id"),
            "if [ -n \"$HORIZON_TEST_AGENT\" ]; then printf '10001\\n'; else printf '0\\n'; fi",
        );
        let sandbox = Self { root };
        fs::create_dir_all(sandbox.path(ISOLATION_MARKER).parent().unwrap()).unwrap();
        fs::write(sandbox.path(ISOLATION_MARKER), "").unwrap();
        fs::create_dir_all(sandbox.state()).unwrap();
        sandbox
    }

    fn path(&self, worker: &str) -> PathBuf {
        self.root.path().join(worker.trim_start_matches('/'))
    }

    /// Where the fake tmux keeps its sessions, buffers and panes.
    fn state(&self) -> PathBuf {
        self.root.path().join("state")
    }

    /// The pane of one session, as its lines.
    fn pane(&self, session: &str) -> String {
        fs::read_to_string(self.state().join("panes").join(session)).unwrap_or_default()
    }

    fn start(&self, session: &str) {
        fs::create_dir_all(self.state().join("panes")).unwrap();
        fs::create_dir_all(self.state().join("sessions")).unwrap();
        fs::write(self.state().join("sessions").join(session), "").unwrap();
        fs::write(self.state().join("panes").join(session), "❯ ").unwrap();
    }

    /// Runs the production remote text for one tmux text, through the sandbox's worker.
    fn run(&self, text: &str) -> Result<Outcome, Failure> {
        let mut sh = Process::new("/bin/sh");
        sh.args(["-c", &self.sandboxed(&cloud_exec::remote(text))]);
        sh.env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()));
        sh.env("HORIZON_TEST_STATE", self.state());
        let started = Instant::now();
        let answer = cloud_exec::execute_ssh(sh, Duration::from_secs(10), &Cancellation::default());
        let outcome = interpret(answer.expect("the sandbox answers"));
        assert!(started.elapsed() < Duration::from_secs(20));
        outcome
    }

    /// The production text with the worker's absolute paths inside this sandbox.
    fn sandboxed(&self, text: &str) -> String {
        text.replace(
            cloud_exec::LAUNCHER,
            &self.path(cloud_exec::LAUNCHER).display().to_string(),
        )
        .replace(ISOLATION_MARKER, &self.path(ISOLATION_MARKER).display().to_string())
    }

    /// The worker's own layer of helpers, which the remote text reaches through `PATH`.
    fn bin(&self) -> PathBuf {
        self.root.path().join("usr/local/bin")
    }

    /// Puts the fake tmux beside the worker's other helpers, so the text finds it as `tmux`.
    fn install(&self) {
        script(&self.bin().join("tmux"), FAKE_TMUX.trim_start_matches("#!/bin/sh\n"));
    }
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A tmux that answers the four calls the tool makes, on files in `$HORIZON_TEST_STATE`. It
/// reads its arguments as a real tmux server would: the socket, the command and the target.
const FAKE_TMUX: &str = r#"#!/bin/sh
set -e
state="$HORIZON_TEST_STATE"
[ "$1" = -L ] || exit 2
[ "$2" = horizon-cloud ] || exit 2
shift 2
command="$1"; shift
target=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -t) target="$2"; shift 2;;
        -S|-N|-b|-s) shift 2;;
        -p|-J|-d) shift;;
        *) shift;;
    esac
done
target="${target#=}"
case "$command" in
    has-session) [ -f "$state/sessions/$target" ];;
    load-buffer) cat > "$state/buffer";;
    paste-buffer) cat "$state/buffer" >> "$state/panes/$target"; rm -f "$state/buffer";;
    send-keys)
        [ "$target" = "" ] && exit 2
        # The agent's user interface prints its prompt again after the key.
        printf '\n❯ ' >> "$state/panes/$target";;
    capture-pane) cat "$state/panes/$target" | tail -n 40;;
    *) exit 2;;
esac
"#;

fn cloud(panel: &str, tmux: &str) -> Deployment {
    let mut record = deployment_record();
    record.sessions = vec![
        Session::new(panel, "claude", "/workspace/agents/panel-1".into()),
        Session::new("panel-2", "shell", "/workspace/sessions/panel-2".into()),
    ];
    for session in &mut record.sessions {
        if session.panel_id == panel {
            tmux.clone_into(&mut session.tmux);
        }
    }
    record
}

/// The smallest record that names sessions, decoded from the shape the store writes.
fn deployment_record() -> Deployment {
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "cloud_id": "cloud-1",
        "repository": "/home/person/shop",
        "revision": "0".repeat(40),
        "profile": {"provider":"hetzner","image":"registry.example/worker","cpu":2,"memory_gb":4,"gpu":false},
        "stage": "Ready",
        "operation": { "state": "bound", "worker_id": "worker1" },
        "spec": null,
        "worker": null,
        "sessions": [],
        "source_ready": true,
    }))
    .unwrap()
}

#[test]
fn an_instruction_reaches_the_terminal_as_text_and_the_answer_shows_it() {
    let sandbox = Sandbox::new();
    sandbox.install();
    sandbox.start("panel-1");
    let record = cloud("panel-1", "panel-1");
    let session = session(&record, "panel-1").unwrap();

    let outcome = sandbox
        .run(&message_text(&session.tmux, "read #1774 and execute PR 1"))
        .unwrap();
    assert_eq!(outcome.uid, 10001, "the write runs as the worker's agent user");
    assert_eq!(outcome.exit_status, 0);
    assert_eq!(
        sandbox.pane("panel-1"),
        "❯ read #1774 and execute PR 1\n❯ ",
        "the terminal shows the instruction the caller sent, once"
    );
    assert!(outcome.text.contains("read #1774 and execute PR 1"), "{}", outcome.text);
    assert!(!outcome.truncated);
}

#[test]
fn text_that_tmux_would_read_as_a_flag_arrives_unchanged() {
    let sandbox = Sandbox::new();
    sandbox.install();
    sandbox.start("panel-1");
    let record = cloud("panel-1", "panel-1");
    let instruction = "-S -t 'other' $(id) `id` ; rm -rf / --no-preserve-root";
    let outcome = sandbox
        .run(&message_text(&record.sessions[0].tmux, instruction))
        .unwrap();
    assert_eq!(outcome.exit_status, 0);
    assert_eq!(sandbox.pane("panel-1"), format!("❯ {instruction}\n❯ "));
}

#[test]
fn a_session_the_worker_does_not_run_is_named_as_such() {
    let sandbox = Sandbox::new();
    sandbox.install();
    // No session is started on the worker, though the cloud's own record has one.
    let record = cloud("panel-1", "panel-1");
    let failure = sandbox.run(&message_text(&record.sessions[0].tmux, "go")).unwrap_err();
    assert_eq!(failure, Failure::NotRunning);
    assert!(failure.to_string().starts_with("cloud_agent_not_running"));
    assert_eq!(sandbox.pane("panel-1"), "", "nothing is typed into an absent session");
    assert_eq!(
        sandbox.run(&read_text(&record.sessions[0].tmux, 40)).unwrap_err(),
        Failure::NotRunning
    );
}

#[test]
fn a_read_answers_the_lines_the_terminal_shows() {
    let sandbox = Sandbox::new();
    sandbox.install();
    sandbox.start("panel-1");
    let record = cloud("panel-1", "panel-1");
    let tmux = &record.sessions[0].tmux;
    sandbox.run(&message_text(tmux, "first")).unwrap();
    sandbox.run(&message_text(tmux, "second")).unwrap();

    let outcome = sandbox.run(&read_text(tmux, 40)).unwrap();
    assert_eq!(outcome.text, "❯ first\n❯ second\n❯ ");
    assert_eq!(outcome.uid, 10001);
    assert_eq!(outcome.exit_status, 0);
}

#[test]
fn a_session_is_found_by_its_panel_and_only_the_worker_runs_a_named_terminal() {
    let record = cloud("panel-1", "panel-1");
    assert_eq!(session(&record, "panel-1").unwrap().tmux, "panel-1");
    assert_eq!(session(&record, "panel-2").unwrap().agent, "shell");
    assert!(
        session(&record, "panel-3").is_none(),
        "a session of another cloud is not found"
    );
    // A record whose terminal name the worker would not accept is not a session at all.
    assert!(session(&cloud("panel-1", "panel 1"), "panel-1").is_none());
    assert!(valid_session_id("panel-1"));
    for invalid in ["", "a b", "a/b", "a'b", &"a".repeat(101)] {
        assert!(!valid_session_id(invalid), "{invalid}");
    }
}

#[test]
fn the_answer_keeps_the_newest_text_and_cuts_its_start_on_a_line() {
    let text = "one\ntwo\nthree\nfour\n";
    assert_eq!(tail(text, 1_000), (text.to_owned(), false));
    assert_eq!(tail(text, 14), ("three\nfour\n".to_owned(), true));
    assert_eq!(tail(text, 5), ("four\n".to_owned(), true));
    // A boundary that falls inside a multi-byte character does not split it.
    let wide = "❯ one\n❯ two\n";
    let (kept, truncated) = tail(wide, 9);
    assert!(truncated);
    assert_eq!(kept, "❯ two\n");
    assert!(kept.is_char_boundary(0));
}

#[test]
fn the_text_names_the_socket_the_session_and_no_other_program() {
    let text = message_text("panel-1", "go");
    assert!(text.contains("tmux -L 'horizon-cloud'"), "{text}");
    assert!(text.contains("-t '=panel-1'"), "the target is exact and quoted: {text}");
    assert!(
        text.contains("load-buffer -"),
        "the text arrives through a buffer: {text}"
    );
    assert!(text.contains("send-keys -t '=panel-1' Enter"), "{text}");
    assert!(text.contains("capture-pane -p -J"), "{text}");
    assert!(!text.contains("sudo") && !text.contains("su "));
    let read = read_text("panel-2", 120);
    assert!(read.contains("-S -120"), "{read}");
    assert!(!read.contains("load-buffer") && !read.contains("send-keys"), "{read}");
}

#[test]
fn a_write_that_tmux_refuses_is_its_own_code_and_not_an_absent_session() {
    let answer = Answer {
        exit_status: WRITE_FAILED,
        uid: 10001,
        stdout: String::new(),
        stderr: String::new(),
        duration: Duration::ZERO,
    };
    assert_eq!(interpret(answer).unwrap_err(), Failure::Write);
    let absent = Answer {
        exit_status: NO_SESSION,
        uid: 10001,
        stdout: String::new(),
        stderr: format!("{NO_SESSION_MARKER}\n"),
        duration: Duration::ZERO,
    };
    assert_eq!(interpret(absent).unwrap_err(), Failure::NotRunning);
    let account = Answer {
        exit_status: 0,
        uid: 10001,
        stdout: "❯\n".to_owned(),
        stderr: String::new(),
        duration: Duration::ZERO,
    };
    assert_eq!(interpret(account).map(|outcome| outcome.text).unwrap(), "❯\n");
}
