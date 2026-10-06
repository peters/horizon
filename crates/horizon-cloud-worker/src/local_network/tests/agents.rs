//! Agents run as their own user (`horizon-agent`, uid 10001 in the worker image), while the
//! helper runs as root. These tests hold agents to the agent socket: it answers every agent
//! operation, and nothing else of the helper's is open to them.
use super::*;
use std::{
    os::unix::{fs::PermissionsExt, net::UnixStream, process::CommandExt},
    process::Command,
    sync::mpsc,
};

/// The agent user in the worker image.
const AGENT_UID: u32 = 10001;
const AGENT_CHILD: &str = "local_network::tests::agents::as_the_agent_user";
const AGENT_SOCKET_VARIABLE: &str = "HORIZON_TEST_LOCAL_NETWORK_AGENT_SOCKET";
const CONTROL_SOCKET_VARIABLE: &str = "HORIZON_TEST_LOCAL_NETWORK_CONTROL_SOCKET";
const HELLO: &str = r#"{"hello":{"discovery":1,"sources":["mdns","probe"]}}"#;
const RETIRE: &str = r#"{"operation":"retire"}"#;

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn printer() -> Device {
    Device {
        address: Ipv4Addr::new(192, 168, 1, 50),
        names: vec!["printer.local".into()],
        services: Vec::new(),
        ports: vec![631],
        sources: vec![Source::Mdns],
    }
}

fn probed() -> discovery::Probe {
    discovery::Probe {
        host: "printer.local".into(),
        address: Ipv4Addr::new(192, 168, 1, 50),
        open: vec![631],
        closed: vec![80],
        silent: Vec::new(),
    }
}

/// Sends one raw request line, as any local process can.
fn ask(socket: &Path, request: &str) -> Answer {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    serde_json::from_str(&read_line(&mut BufReader::new(stream)).unwrap().unwrap()).unwrap()
}

/// Starts a session whose owner's Horizon discovers and probes.
fn start(paths: &Paths) -> Session {
    let mut session = Session::start(paths, NONCE);
    session.tell(HELLO);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !status(paths).unwrap().discovery.unwrap().probe {
        assert!(Instant::now() < deadline, "no hello");
        thread::sleep(Duration::from_millis(20));
    }
    session
}

/// Passes the helper's calls to the owner on, so the test can answer them while it waits.
fn owner_calls(session: &mut Session) -> mpsc::Receiver<Call> {
    let (idle, _) = io::pipe().unwrap();
    let mut output = std::mem::replace(&mut session.output, BufReader::new(idle));
    let (sender, calls) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(Some(line)) = read_line(&mut output) {
            if let Ok(call) = serde_json::from_str(&line)
                && sender.send(call).is_err()
            {
                break;
            }
        }
    });
    calls
}

/// Answers as the owner's Horizon until the agent's work is done.
fn answer_owner(session: &mut Session, calls: &mpsc::Receiver<Call>, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "the agent did not finish");
        let Ok(call) = calls.recv_timeout(Duration::from_millis(50)) else {
            continue;
        };
        let answer = match call.request {
            discovery::Request::Discover => discovery::Answer::Discovery(discovery::Discovery {
                devices: vec![printer()],
                ..discovery::Discovery::default()
            }),
            discovery::Request::Probe { .. } => discovery::Answer::Probe(probed()),
        };
        session.tell(&serde_json::to_string(&discovery::Message::Answer { id: call.id, answer }).unwrap());
    }
}

/// Everything an agent may do, through the agent socket alone.
fn agent_operations(paths: &Paths) {
    let current = status(paths).unwrap();
    assert!(current.active, "{current:?}");
    assert_eq!(current.subnet.as_deref(), Some("192.168.1.0/24"));
    assert!(current.proxy.is_some());
    assert_eq!(discover(paths).unwrap().devices, [printer()]);
    assert_eq!(probe(paths, "printer.local", vec![631, 80]).unwrap(), probed());
    let device = forward(paths, "192.168.1.50", 554).unwrap();
    assert!(echoes(device.worker_port));
    assert_eq!(status(paths).unwrap().forwards, std::slice::from_ref(&device));
    assert!(unforward(paths, device.worker_port).unwrap().forwards.is_empty());
    // Only a newer session's helper may ask the running one to make way.
    let refused = ask(&paths.agent, RETIRE);
    assert!(
        matches!(&refused, Answer::Error(error) if error.contains("Only a bridge helper")),
        "{refused:?}"
    );
    assert!(status(paths).unwrap().active);
}

#[test]
fn agents_need_only_the_agent_socket_and_cannot_retire_the_helper() {
    let (root, paths) = paths();
    let mut session = start(&paths);
    // Every local user may connect to the agent socket; the rest stays the helper's.
    assert_eq!(mode(&paths.agent), 0o666);
    assert_eq!(mode(&paths.directory), 0o700);
    assert_eq!(mode(&paths.control()), 0o600);
    // An agent client that knows nothing of the private directory still does everything.
    let agent = Paths {
        directory: root.path().join("absent"),
        agent: paths.agent.clone(),
    };
    let calls = owner_calls(&mut session);
    let client = thread::spawn(move || agent_operations(&agent));
    answer_owner(&mut session, &calls, || client.is_finished());
    client.join().unwrap();
    // The private socket still takes a retire request, and a live owner keeps the bridge.
    let kept = ask(&paths.control(), RETIRE);
    assert!(
        matches!(&kept, Answer::Error(error) if error.contains("Another Horizon")),
        "{kept:?}"
    );
    session.stop();
    assert!(!paths.agent.exists());
    assert!(!status(&paths).unwrap().active);
}

#[test]
fn agents_that_fill_their_socket_leave_the_private_socket_answering() {
    let (_root, paths) = paths();
    let mut session = Session::start(&paths, NONCE);
    // Requests that never finish hold every share of the agent socket.
    let held: Vec<_> = (0..8).map(|_| UnixStream::connect(&paths.agent).unwrap()).collect();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let stream = UnixStream::connect(&paths.agent).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        // An empty request is answered at once, so a check made too early frees its share.
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut reader = BufReader::new(stream);
        let line = read_line(&mut reader).unwrap().unwrap_or_default();
        if line.contains("busy") {
            break;
        }
        assert!(Instant::now() < deadline, "the agent socket never filled: {line}");
        thread::sleep(Duration::from_millis(20));
    }
    let answer = ask(&paths.control(), r#"{"operation":"status"}"#);
    assert!(matches!(&answer, Answer::Status(status) if status.active), "{answer:?}");
    drop(held);
    session.stop();
}

#[test]
fn an_agent_socket_the_caller_cannot_use_is_an_error_not_an_off_bridge() {
    let (_root, paths) = paths();
    let _listener = UnixListener::bind(&paths.agent).unwrap();
    std::fs::set_permissions(&paths.agent, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root ignores the mode, so only another user sees the refusal.
    if rustix::process::geteuid().is_root() {
        return;
    }
    let error = status(&paths).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(
        error.to_string().contains("Cannot reach the Local Network Bridge"),
        "{error}"
    );
}

#[test]
fn a_request_that_trickles_in_loses_its_share_after_the_deadline() {
    let (_root, paths) = paths();
    let mut session = Session::start(&paths, NONCE);
    let mut stream = UnixStream::connect(&paths.agent).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let started = Instant::now();
    let trickle = {
        let mut stream = stream.try_clone().unwrap();
        thread::spawn(move || {
            // One byte at a time, never a whole line, until the helper hangs up.
            while started.elapsed() < Duration::from_secs(20) && stream.write_all(b" ").is_ok() {
                thread::sleep(Duration::from_millis(500));
            }
        })
    };
    let mut rest = Vec::new();
    let ended = stream.read_to_end(&mut rest);
    assert!(ended.is_ok(), "{ended:?}");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(9) && elapsed < Duration::from_secs(15),
        "{elapsed:?}"
    );
    trickle.join().unwrap();
    assert!(status(&paths).unwrap().active);
    session.stop();
}

/// Runs the agent operations as the agent user itself. Only root can switch users, so run
/// it as root, for example in a container:
/// `cargo test -p horizon-cloud-worker -- --ignored --exact local_network::tests::agents::the_agent_user_reaches_the_agent_operations_and_not_the_private_socket`.
#[test]
#[ignore = "needs root to run a client as the agent user, uid 10001"]
fn the_agent_user_reaches_the_agent_operations_and_not_the_private_socket() {
    assert!(
        rustix::process::geteuid().is_root(),
        "only root can run a client as uid {AGENT_UID}"
    );
    let (root, paths) = paths();
    // The agent user may enter the test root, as it may enter /run on a worker, and runs a
    // copy of this test program from there: the original may sit in a private home.
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let program = root.path().join("worker-tests");
    std::fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut session = start(&paths);
    let calls = owner_calls(&mut session);
    let child = {
        let (agent, control) = (paths.agent.clone(), paths.control());
        thread::spawn(move || {
            Command::new(program)
                .args(["--exact", AGENT_CHILD, "--ignored", "--nocapture", "--test-threads=1"])
                .env(AGENT_SOCKET_VARIABLE, agent)
                .env(CONTROL_SOCKET_VARIABLE, control)
                .uid(AGENT_UID)
                .gid(AGENT_UID)
                .output()
        })
    };
    answer_owner(&mut session, &calls, || child.is_finished());
    let output = child.join().unwrap();
    let still_active = status(&paths).map(|status| status.active);
    session.stop();
    let output = output.unwrap();
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{report}");
    assert!(report.contains("1 passed"), "{report}");
    assert!(still_active.unwrap());
}

/// The client half of the test above, run as uid 10001.
#[test]
#[ignore = "run as the agent user by the_agent_user_reaches_the_agent_operations_and_not_the_private_socket"]
fn as_the_agent_user() {
    let (Some(agent), Some(control)) = (
        std::env::var_os(AGENT_SOCKET_VARIABLE),
        std::env::var_os(CONTROL_SOCKET_VARIABLE),
    ) else {
        eprintln!("nothing to do: the uid {AGENT_UID} test above starts this one");
        return;
    };
    let control = PathBuf::from(control);
    assert_eq!(rustix::process::geteuid().as_raw(), AGENT_UID);
    assert_eq!(
        UnixStream::connect(&control).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    let paths = Paths {
        directory: control.parent().unwrap().to_owned(),
        agent: agent.into(),
    };
    agent_operations(&paths);
}
