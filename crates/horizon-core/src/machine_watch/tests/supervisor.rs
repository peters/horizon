//! The supervisor of the watch over a stand-in `ssh` that runs the text of the
//! watch on this computer: a start, a lost link that comes back, a refusal, a stop
//! with no child left, and the limit of watched machines.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::MachineKeys;
use crate::machine_watch::{MachineWatch, REMOTE_PROGRAM, WatchLink, WatchRefusal};
use crate::machines::{Machine, MachineRegistry, Machines};
use crate::util::stand_in_program_in;

use super::support::{Server, has};

/// How long a test waits for the snapshot of the watch to say what it expects.
const DEADLINE: Duration = Duration::from_secs(25);

/// How often a test reads the snapshot while it waits.
const POLL: Duration = Duration::from_millis(20);

/// The delay that a lane of this module gives a lost link: the rule of a lost link is
/// proven in `crate::place`, and a lane here waits milliseconds for its next attempt
/// instead of the seconds that a person waits.
fn short_delay(_attempt: u32) -> Duration {
    Duration::from_millis(40)
}

/// One machine record of a test.
fn record(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "127.0.0.1".to_owned(),
        user: None,
        port: Some(2222),
        session: None,
        provider: None,
        repository: None,
        folder: None,
        added_at_millis: 0,
        cards: false,
    }
}

/// The program of an agent pane of a test: a script whose file name is `codex`,
/// which a shell runs, so the pane reports the command line of its interpreter.
fn agent_program() -> Option<(tempfile::TempDir, PathBuf)> {
    let folder = tempfile::tempdir().ok()?;
    let program = stand_in_program_in(folder.path(), "codex", "#!/bin/sh\nsleep 120\n").ok()?;
    Some((folder, program))
}

/// The machine list of a test with `names`, in a folder of its own.
fn machines_of(folder: &Path, names: &[&str]) -> (Machines, MachineKeys) {
    let registry = MachineRegistry::new(folder.to_path_buf());
    let records: Vec<Machine> = names.iter().map(|name| record(name)).collect();
    (
        Machines::with_records(registry, records),
        MachineKeys::new(folder.join("machine-keys")),
    )
}

/// One stand-in `ssh` of a test: the count of its calls, the argv of its first call
/// and the marker of its children.
struct StandIn {
    folder: tempfile::TempDir,
    calls: PathBuf,
    options: PathBuf,
    marker: String,
}

impl StandIn {
    /// The stand-in that runs the text of the watch on the private server of
    /// `server`: the one word of the remote command reads that text on the input of
    /// the stand-in, and `-s` keeps the marker of this test in the process list.
    fn watch(server: &Server) -> Option<Self> {
        Self::of(server, "exec $last -s \"$marker\"")
    }

    /// The stand-in that runs `body` after it counted its call. The last argument of
    /// the call is in `$last`, which a one-word command of a login shell names; the
    /// marker of its child is in `$marker`.
    fn of(server: &Server, body: &str) -> Option<Self> {
        if !has("sh") {
            return None;
        }
        let folder = tempfile::tempdir().ok()?;
        let calls = folder.path().join("calls");
        let options = folder.path().join("options");
        let marker = format!("hzw-child-{}-{}", std::process::id(), server.socket());
        let body = format!(
            "#!/bin/sh\nprintf 'c' >> {calls}\n\
             [ -f {options} ] || printf '%s\\n' \"$*\" | tr '\\n' ' ' > {options}\n\
             marker={marker}\nfor a in \"$@\"; do last=$a; done\n\
             HORIZON_MACHINE_WATCH_TMUX_SOCKET={socket}\nexport HORIZON_MACHINE_WATCH_TMUX_SOCKET\n{body}\n",
            calls = crate::util::quote(&calls.to_string_lossy()),
            options = crate::util::quote(&options.to_string_lossy()),
            socket = crate::util::quote(server.socket()),
        );
        stand_in_program_in(folder.path(), "ssh", &body).ok()?;
        Some(Self {
            folder,
            calls,
            options,
            marker,
        })
    }

    /// The program of the stand-in.
    fn program(&self) -> String {
        let path = self.folder.path().join("ssh");
        path.to_string_lossy().into_owned()
    }

    /// How many times the watch started this program.
    fn calls(&self) -> usize {
        std::fs::metadata(&self.calls).map_or(0, |held| usize::try_from(held.len()).unwrap_or(0))
    }

    /// The command line of the first call of the watch.
    fn first_call(&self) -> String {
        std::fs::read_to_string(&self.options).unwrap_or_default()
    }

    /// How many `sh` children of this stand-in run now.
    fn children(&self) -> usize {
        let output = std::process::Command::new("ps")
            .args(["-A", "-o", "args="])
            .output()
            .expect("the process list of this computer");
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines().filter(|line| line.contains(&self.marker)).count()
    }
}

/// Waits until `wanted` holds, and names what the test waited for.
fn wait_until(what: &str, wanted: impl Fn() -> bool) {
    wait_until_within(DEADLINE, what, wanted);
}

/// Waits until `wanted` holds within `limit`, and names what the test waited for.
fn wait_until_within(limit: Duration, what: &str, wanted: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !wanted() {
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(POLL);
    }
}

/// Waits until the snapshot of `watch` answers `wanted` for `machine`.
fn wait_for(watch: &MachineWatch, machine: &str, wanted: impl Fn(WatchLink) -> bool) -> WatchLink {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let snapshot = watch.snapshot();
        let link = snapshot.machine(machine).map(|state| state.link);
        if let Some(link) = link
            && wanted(link)
        {
            return link;
        }
        assert!(
            Instant::now() < deadline,
            "{machine} did not reach the link of the test: {link:?}"
        );
        std::thread::sleep(POLL);
    }
}

#[test]
fn the_command_line_of_a_watch_holds_the_options_of_a_machine_terminal() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let target = crate::machine_watch::WatchTarget::of(&machines, &keys, "build-1").expect("the watch of the machine");
    assert_eq!(target.machine, "build-1");
    assert_eq!(
        target.args.last().map(String::as_str),
        Some(REMOTE_PROGRAM),
        "the machine runs the one word that no login shell has to read"
    );
    for option in [
        "BatchMode=yes",
        "ConnectTimeout=10",
        "ServerAliveInterval=5",
        "ServerAliveCountMax=3",
    ] {
        assert!(
            target.args.iter().any(|argument| argument == option),
            "the command line holds {option}: {:?}",
            target.args
        );
    }
    assert!(
        !target.args.iter().any(|argument| argument == "-t"),
        "a watch asks for no terminal: {:?}",
        target.args
    );
    assert_eq!(target.args.first().map(String::as_str), Some("-p"));
    assert_eq!(target.args.get(1).map(String::as_str), Some("2222"));

    let (machines, keys) = machines_of(folder.path(), &["build-2"]);
    let machines = machines.with_key("build-2", "key-1");
    let target = crate::machine_watch::WatchTarget::of(&machines, &keys, "build-2").expect("the watch of the machine");
    let known_hosts = target
        .args
        .iter()
        .find(|argument| argument.starts_with("UserKnownHostsFile="))
        .unwrap_or_else(|| panic!("the known-hosts file of the machine key: {:?}", target.args));
    assert!(
        known_hosts.starts_with("UserKnownHostsFile=") && known_hosts.ends_with(".known_hosts"),
        "the known-hosts file of this key: {known_hosts}"
    );
    assert!(
        target.args.iter().any(|argument| argument.ends_with("key-1")),
        "the key file of the binding: {:?}",
        target.args
    );
    assert!(
        target.args.iter().any(|argument| argument == "IdentitiesOnly=yes"),
        "the watch offers this key alone"
    );
    assert!(
        target.args.iter().any(|argument| argument == "IdentityAgent=none"),
        "the watch offers no agent of the person"
    );
}

#[test]
fn a_record_that_breaks_the_rule_of_a_terminal_is_refused_and_starts_no_child() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let mut broken = record("build-1");
    broken.host = "not a host".to_owned();
    let registry = MachineRegistry::new(folder.path().to_path_buf());
    let machines = Machines::with_records(registry, vec![broken]);
    let keys = MachineKeys::new(folder.path().join("machine-keys"));
    let watch = MachineWatch::with_program(folder.path().join("ssh").to_string_lossy().into_owned());
    watch.reconcile(&machines, &keys, true);
    let link = wait_for(&watch, "build-1", |link| matches!(link, WatchLink::Refused { .. }));
    let WatchLink::Refused { reason, .. } = link else {
        panic!("the refusal of the record: {link:?}");
    };
    assert_eq!(reason, WatchRefusal::Record, "the record is no route of a terminal");
}

#[test]
fn a_watch_reports_the_agents_of_its_machine_and_marks_one_seen() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(stand_in) = StandIn::watch(&server) else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let Some((_program_folder, agent)) = agent_program() else {
        return;
    };
    server.session("hz-codex-1", &agent.display().to_string());
    let watch = MachineWatch::with_program(stand_in.program());
    watch.reconcile(&machines, &keys, true);
    let link = wait_for(&watch, "build-1", WatchLink::watching);
    assert_eq!(link, WatchLink::Watching, "the machine answered with a report");
    wait_until("the ssh child of the watch", || stand_in.calls() >= 1);
    assert!(
        stand_in.first_call().contains("BatchMode=yes"),
        "the child received the options of a watch: {:?}",
        stand_in.first_call()
    );
    let snapshot = watch.snapshot();
    let state = snapshot.machine("build-1").expect("the machine of the report");
    assert_eq!(state.agents.len(), 1, "one agent pane of the server: {state:?}");
    assert_eq!(state.agents[0].session, "hz-codex-1");
    assert_eq!(snapshot.needs_you + snapshot.done, 0);
    let version = snapshot.version;
    assert!(
        !watch.mark_seen("build-1", &state.agents[0].session, &state.agents[0].pane),
        "an agent that the person saw as idle changes no row"
    );
    assert_eq!(watch.snapshot().version, version, "a look of nothing does no work");
    watch.stop_all();
    assert!(watch.snapshot().machines.is_empty(), "a stop forgets every machine");
}

#[test]
fn a_login_shell_that_is_not_posix_runs_the_watch_through_its_input() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(stand_in) = non_posix_login_shell(&server) else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let Some((_program_folder, agent)) = agent_program() else {
        return;
    };
    server.session("hz-codex-1", &agent.display().to_string());
    let watch = MachineWatch::with_program(stand_in.program());
    watch.reconcile(&machines, &keys, true);
    let link = wait_for(&watch, "build-1", WatchLink::watching);
    assert_eq!(
        link,
        WatchLink::Watching,
        "a login shell that is not POSIX runs the watch through its input"
    );
    let snapshot = watch.snapshot();
    assert_eq!(
        snapshot
            .machine("build-1")
            .expect("the machine of the report")
            .agents
            .len(),
        1,
        "the report arrives, and it names the agent pane of the server"
    );
    watch.stop_all();
}

/// One stand-in `ssh` whose last argument a login shell that is not POSIX reads: it
/// refuses a word that is not one plain word, as such a shell cannot run a POSIX text,
/// and it runs one plain word with its own input.
fn non_posix_login_shell(server: &Server) -> Option<StandIn> {
    StandIn::of(
        server,
        "case $last in ''|*[!A-Za-z0-9_./-]*)\n  printf 'this login shell runs one word\\n' >&2\n  exit 1\n  ;;\nesac\nexec \"$last\"",
    )
}

#[test]
fn a_link_that_drops_comes_back_by_the_rule_of_a_lost_link() {
    let Some(server) = Server::new() else {
        return;
    };
    let calls = tempfile::tempdir().expect("a folder of the test");
    let counter = calls.path().join("calls");
    let Some(stand_in) = StandIn::of(
        &server,
        &format!(
            "if [ -f {counter} ]; then exec $last -s \"$marker\"; fi\n\
             printf 'x' > {counter}\nprintf 'HZ\\t1\\nZ\\n'\nexit 255",
            counter = crate::util::quote(&counter.to_string_lossy()),
        ),
    ) else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let mut watch = MachineWatch::with_program(stand_in.program());
    watch.delay = short_delay;
    let started = Instant::now();
    watch.reconcile(&machines, &keys, true);
    wait_for(&watch, "build-1", |link| matches!(link, WatchLink::Lost { .. }));
    let link = wait_for(&watch, "build-1", WatchLink::watching);
    assert_eq!(link, WatchLink::Watching, "the second attempt reached the machine");
    assert!(
        started.elapsed() >= short_delay(1),
        "the watch waited the delay that it gives a lost link: {:?}",
        started.elapsed()
    );
    assert_eq!(
        (MachineWatch::default().delay)(1),
        crate::place::reconnect_delay(1),
        "the watch of a person waits the delay of the rule of a lost link"
    );
    wait_until("the second attempt of the watch", || stand_in.calls() >= 2);
    assert_eq!(stand_in.calls(), 2, "two attempts of one watch");
}

/// One stand-in `ssh` that ends its first call with `end` and runs the text of the
/// watch from its second call, with the folder that holds the phase of the two calls.
fn stand_in_that_ends(server: &Server, end: &str) -> Option<(StandIn, tempfile::TempDir)> {
    let phase = tempfile::tempdir().ok()?;
    let counter = phase.path().join("phase");
    let stand_in = StandIn::of(
        server,
        &format!(
            "if [ -f {counter} ]; then exec $last -s \"$marker\"; fi\n\
             printf 'x' > {counter}\nprintf 'HZ\\t1\\nZ\\n'\n{end}",
            counter = crate::util::quote(&counter.to_string_lossy()),
        ),
    )?;
    Some((stand_in, phase))
}

#[test]
fn a_child_that_ends_with_its_own_status_is_a_lost_link_that_comes_back() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some((stand_in, _phase)) = stand_in_that_ends(&server, "exit 1") else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let mut watch = MachineWatch::with_program(stand_in.program());
    watch.delay = short_delay;
    watch.reconcile(&machines, &keys, true);
    wait_for(&watch, "build-1", |link| matches!(link, WatchLink::Lost { .. }));
    wait_until("the second attempt of the watch", || stand_in.calls() >= 2);
    assert_eq!(
        wait_for(&watch, "build-1", WatchLink::watching),
        WatchLink::Watching,
        "the machine answered after the child ended with a status of its own"
    );
    watch.stop_all();
}

#[test]
fn a_child_that_a_signal_ends_is_a_lost_link_that_comes_back() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some((stand_in, _phase)) = stand_in_that_ends(&server, "kill -TERM $$") else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let mut watch = MachineWatch::with_program(stand_in.program());
    watch.delay = short_delay;
    watch.reconcile(&machines, &keys, true);
    wait_for(&watch, "build-1", |link| matches!(link, WatchLink::Lost { .. }));
    wait_until("the second attempt of the watch", || stand_in.calls() >= 2);
    assert_eq!(
        wait_for(&watch, "build-1", WatchLink::watching),
        WatchLink::Watching,
        "the machine answered after a signal ended the child"
    );
    watch.stop_all();
}

#[test]
fn a_refused_host_key_is_a_typed_refusal_of_the_snapshot() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(stand_in) = StandIn::of(&server, "printf 'Host key verification failed.\\r\\n' >&2\nexit 255") else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let watch = MachineWatch::with_program(stand_in.program());
    watch.reconcile(&machines, &keys, true);
    let link = wait_for(&watch, "build-1", |link| matches!(link, WatchLink::Refused { .. }));
    let WatchLink::Refused { reason, .. } = link else {
        panic!("the refusal of the host key: {link:?}");
    };
    assert_eq!(reason, WatchRefusal::HostKey);
}

#[test]
fn a_refused_key_is_a_typed_refusal_of_the_snapshot() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(stand_in) = StandIn::of(
        &server,
        "printf 'person@127.0.0.1: Permission denied (publickey).\\r\\n' >&2\nexit 255",
    ) else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let watch = MachineWatch::with_program(stand_in.program());
    watch.reconcile(&machines, &keys, true);
    let link = wait_for(&watch, "build-1", |link| matches!(link, WatchLink::Refused { .. }));
    let WatchLink::Refused { reason, .. } = link else {
        panic!("the refusal of the key: {link:?}");
    };
    assert_eq!(reason, WatchRefusal::Key, "the words of ssh name a refused key");
}

#[test]
fn a_stop_leaves_no_child_of_the_watch() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(stand_in) = StandIn::watch(&server) else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let watch = MachineWatch::with_program(stand_in.program());
    watch.reconcile(&machines, &keys, true);
    wait_for(&watch, "build-1", WatchLink::watching);
    assert_eq!(stand_in.children(), 1, "one ssh child of one watch");
    let started = Instant::now();
    watch.stop_all();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the stop of one watch is bounded: {:?}",
        started.elapsed()
    );
    assert_eq!(stand_in.children(), 0, "the stop left no child of the watch");
}

#[test]
fn the_watch_holds_eight_machines_and_the_ninth_says_so() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(stand_in) = StandIn::watch(&server) else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let names: Vec<String> = (1..=9).map(|number| format!("build-{number}")).collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    let (machines, keys) = machines_of(folder.path(), &borrowed);
    let watch = MachineWatch::with_program(stand_in.program());
    watch.reconcile(&machines, &keys, true);
    wait_for(&watch, "build-8", WatchLink::watching);
    assert_eq!(
        wait_for(&watch, "build-9", |link| link == WatchLink::OverTheLimit),
        WatchLink::OverTheLimit,
        "the ninth machine is not watched"
    );
    assert_eq!(
        watch.snapshot().machines.len(),
        9,
        "every machine of the list is in the snapshot"
    );
    watch.stop_all();
    assert_eq!(
        stand_in.calls(),
        crate::machine_watch::MAX_WATCHED_MACHINES,
        "one ssh child per watched machine and none for the ninth"
    );
    watch.stop_all();
}

#[test]
fn the_setting_off_stops_every_watch_and_says_so() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(stand_in) = StandIn::watch(&server) else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1", "build-2"]);
    let watch = MachineWatch::with_program(stand_in.program());
    watch.reconcile(&machines, &keys, true);
    // Both watches are up and report, so the setting finds two readers that run.
    for machine in ["build-1", "build-2"] {
        wait_for(&watch, machine, WatchLink::watching);
    }
    watch.reconcile(&machines, &keys, false);
    for machine in ["build-1", "build-2"] {
        assert_eq!(
            wait_for(&watch, machine, |link| link == WatchLink::Off),
            WatchLink::Off,
            "the setting off is the link of {machine}"
        );
        assert!(
            watch.snapshot().agents(machine).is_empty(),
            "a machine whose watch is off holds no agent"
        );
    }
    assert_eq!(stand_in.children(), 0, "the setting off left no child");
}

#[test]
fn a_frame_reads_the_snapshot_without_a_wait() {
    crate::ui_thread::use_strict_mode();
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (machines, keys) = machines_of(folder.path(), &["build-1"]);
    let watch = MachineWatch::with_program(folder.path().join("ssh").to_string_lossy().into_owned());
    watch.reconcile(&machines, &keys, false);
    {
        let frame = crate::ui_thread::FrameScope::enter();
        let first = watch.snapshot();
        let second = watch.snapshot();
        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "a frame holds one value of the snapshot and copies no machine"
        );
        assert_eq!(first.version, second.version);
        assert!(!watch.mark_seen("build-1", "hz-1", "%0"));
        assert_eq!(first.machine("build-1").map(|state| state.link), Some(WatchLink::Off));
        drop(frame);
    }
}
