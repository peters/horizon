use super::*;
use std::{
    net::{Ipv4Addr, TcpListener, TcpStream},
    os::unix::net::UnixListener,
    path::Path,
    thread,
    time::Instant,
};

const NONCE: &str = "0123456789abcdef0123456789abcdef";

/// Stands in for the owner's proxy behind the bridge socket: it admits `192.168.1.50` and
/// relays that to a local echo server, and refuses every other destination as out of scope.
fn owner_proxy(socket: &Path) -> JoinHandle {
    let listener = UnixListener::bind(socket).unwrap();
    let echo = TcpListener::bind("127.0.0.1:0").unwrap();
    let echo_address = echo.local_addr().unwrap();
    thread::spawn(move || {
        for mut socket in echo.incoming().flatten() {
            thread::spawn(move || {
                let mut buffer = [0; 1024];
                while let Ok(count) = socket.read(&mut buffer) {
                    if count == 0 || socket.write_all(&buffer[..count]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            thread::spawn(move || {
                let mut greeting = [0; 3];
                if stream.read_exact(&mut greeting).is_err() {
                    return;
                }
                stream.write_all(&[5, 0]).unwrap();
                let mut request = [0; 10];
                if stream.read_exact(&mut request).is_err() {
                    return;
                }
                let admitted = request[3] == 1 && request[4..8] == [192, 168, 1, 50];
                stream
                    .write_all(&[5, if admitted { 0 } else { 2 }, 0, 1, 0, 0, 0, 0, 0, 0])
                    .unwrap();
                if admitted {
                    let echo = TcpStream::connect(echo_address).unwrap();
                    let mut back = echo.try_clone().unwrap();
                    let mut forth = stream.try_clone().unwrap();
                    thread::spawn(move || io::copy(&mut back, &mut forth));
                    let mut echo = echo;
                    let _ = io::copy(&mut stream, &mut echo);
                }
            });
        }
    })
}

type JoinHandle = thread::JoinHandle<()>;

struct Session {
    heartbeat: Option<io::PipeWriter>,
    /// The helper's output after its ready line: its calls to the owner.
    output: BufReader<io::PipeReader>,
    ready: Ready,
    helper: Option<thread::JoinHandle<io::Result<()>>>,
}

impl Session {
    fn start(paths: &Paths, nonce: &str) -> Self {
        let nonce_value = Nonce::parse(nonce).unwrap();
        prepare(paths).unwrap();
        owner_proxy(&paths.bridge(&nonce_value));
        let (input, heartbeat) = io::pipe().unwrap();
        let (output, ready_writer) = io::pipe().unwrap();
        let helper = {
            let (paths, nonce) = (paths.clone(), nonce.to_owned());
            thread::spawn(move || hold::run(&paths, &nonce, "192.168.1.0/24", input, ready_writer))
        };
        let mut output = BufReader::new(output);
        let line = read_line(&mut output).unwrap().unwrap();
        Self {
            heartbeat: Some(heartbeat),
            output,
            ready: serde_json::from_str(&line).unwrap(),
            helper: Some(helper),
        }
    }

    fn stop(&mut self) {
        drop(self.heartbeat.take());
        self.helper.take().unwrap().join().unwrap().unwrap();
    }
}

use horizon_cloud_protocol::local_network::{
    Ready,
    discovery::{self, Call, Device, MAX_LINE, Source},
};

impl Session {
    /// Writes one line as the owner's Horizon.
    fn tell(&mut self, line: &str) {
        let owner = self.heartbeat.as_mut().unwrap();
        owner.write_all(line.as_bytes()).unwrap();
        owner.write_all(b"\n").unwrap();
    }

    /// Waits for the helper's next call to the owner.
    fn call(&mut self) -> Call {
        serde_json::from_str(&read_line(&mut self.output).unwrap().unwrap()).unwrap()
    }
}

#[test]
fn discovery_asks_the_owner_and_bad_owner_lines_never_end_the_session() {
    let (_root, paths) = paths();
    let mut session = Session::start(&paths, NONCE);
    let hello =
        r#"{"hello":{"discovery":1,"sources":["mdns","ssdp","neighbors"],"note":"Not qualified\u001b[2J here"}}"#;
    // Before the owner says what it answers, discovery is unavailable.
    assert!(!status(&paths).unwrap().discovery.unwrap().available);
    let refused = discover(&paths).unwrap_err().to_string();
    assert!(refused.contains("update Horizon"), "{refused}");
    // Junk, and a hello at the end of an oversized line, are ignored.
    session.tell("not json");
    session.tell(&format!("{}{hello}", "x".repeat(MAX_LINE)));
    session.tell(r#"{"answer":{"id":99,"answer":{"refused":"nobody asked"}}}"#);
    thread::sleep(Duration::from_millis(200));
    assert!(!status(&paths).unwrap().discovery.unwrap().available);
    session.tell(hello);
    let deadline = Instant::now() + Duration::from_secs(5);
    let availability = loop {
        let availability = status(&paths).unwrap().discovery.unwrap();
        if availability.available || Instant::now() > deadline {
            break availability;
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        availability,
        Availability {
            available: true,
            probe: false,
            sources: vec!["mdns".into(), "ssdp".into(), "neighbors".into()],
            note: Some("Not qualified[2J here".into()),
        }
    );

    let asking = {
        let paths = paths.clone();
        thread::spawn(move || discover(&paths))
    };
    let call = session.call();
    assert_eq!(call.request, discovery::Request::Discover);
    let printer = Device {
        address: Ipv4Addr::new(192, 168, 1, 50),
        names: vec!["printer.local".into()],
        services: Vec::new(),
        ports: vec![631],
        sources: vec![Source::Mdns],
    };
    let answer =
        |id: u64, answer: discovery::Answer| serde_json::to_string(&discovery::Message::Answer { id, answer }).unwrap();
    // An answer to another call does not end this one.
    session.tell(&answer(call.id + 1, discovery::Answer::Refused("wrong".into())));
    session.tell(&answer(
        call.id,
        discovery::Answer::Discovery(discovery::Discovery {
            devices: vec![printer.clone()],
            ..discovery::Discovery::default()
        }),
    ));
    assert_eq!(asking.join().unwrap().unwrap().devices, [printer]);

    let asking = {
        let paths = paths.clone();
        thread::spawn(move || discover(&paths))
    };
    let call = session.call();
    session.tell(&answer(call.id, discovery::Answer::Refused("Busy browsing".into())));
    let refused = asking.join().unwrap().unwrap_err().to_string();
    assert_eq!(refused, "Busy browsing");
    assert!(status(&paths).unwrap().active);
    session.stop();
}

fn paths() -> (tempfile::TempDir, Paths) {
    // Unix socket paths are limited to about 100 bytes.
    let root = tempfile::Builder::new().prefix("lnw").tempdir_in("/tmp").unwrap();
    let paths = Paths {
        directory: root.path().join("run"),
    };
    (root, paths)
}

fn echoes(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect((Ipv4Addr::LOCALHOST, port)) else {
        return false;
    };
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut echoed = [0; 5];
    stream.write_all(b"frame").is_ok() && stream.read_exact(&mut echoed).is_ok() && &echoed == b"frame"
}

#[test]
fn a_session_serves_status_the_proxy_and_forwards_until_its_input_ends() {
    let (_root, paths) = paths();
    let mut session = Session::start(&paths, NONCE);
    let current = status(&paths).unwrap();
    assert!(current.active);
    assert_eq!(current.subnet.as_deref(), Some("192.168.1.0/24"));
    assert_eq!(current.proxy, Some(session.ready.proxy.to_string()));
    assert!(current.forwards.is_empty());

    // The SOCKS endpoint is a plain relay onto the bridge socket.
    let mut raw = TcpStream::connect(session.ready.proxy).unwrap();
    raw.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    raw.write_all(&[5, 1, 0]).unwrap();
    let mut choice = [0; 2];
    raw.read_exact(&mut choice).unwrap();
    assert_eq!(choice, [5, 0]);

    let device = forward(&paths, "192.168.1.50", 554).unwrap();
    assert_eq!((device.host.as_str(), device.port), ("192.168.1.50", 554));
    assert!(echoes(device.worker_port));
    assert_eq!(forward(&paths, "192.168.1.50", 554).unwrap(), device);
    let refused = forward(&paths, "10.0.0.1", 80).unwrap_err().to_string();
    assert!(refused.contains("Outside the bridged local network"), "{refused}");
    assert_eq!(status(&paths).unwrap().forwards, vec![device.clone()]);

    assert!(unforward(&paths, device.worker_port).unwrap().forwards.is_empty());
    let missing = unforward(&paths, device.worker_port).unwrap_err().to_string();
    assert!(missing.contains("No forward"), "{missing}");

    forward(&paths, "192.168.1.50", 554).unwrap();
    session.stop();
    assert!(!status(&paths).unwrap().active);
    assert!(!paths.control().exists());
    assert!(!paths.bridge(&Nonce::parse(NONCE).unwrap()).exists());
    assert!(
        forward(&paths, "192.168.1.50", 554)
            .unwrap_err()
            .to_string()
            .contains("off")
    );
}

#[test]
fn a_live_session_keeps_the_bridge_and_a_dead_one_makes_way() {
    let (_root, paths) = paths();
    let mut older = Session::start(&paths, NONCE);
    let newer = "fedcba9876543210fedcba9876543210";
    owner_proxy(&paths.bridge(&Nonce::parse(newer).unwrap()));
    let refused = hold::run(&paths, newer, "192.168.1.0/24", io::empty(), io::sink()).unwrap_err();
    assert!(refused.to_string().contains("Another Horizon"), "{refused}");
    assert_eq!(status(&paths).unwrap().proxy, Some(older.ready.proxy.to_string()));
    older.stop();
    assert!(!status(&paths).unwrap().active);
    // Something that answers nothing on the control socket is not taken over.
    let silent = UnixListener::bind(paths.control()).unwrap();
    let hang_up = thread::spawn(move || drop(silent.accept()));
    // Each refused session removed its bridge socket, as a real one would.
    owner_proxy(&paths.bridge(&Nonce::parse(newer).unwrap()));
    let unanswered = hold::run(&paths, newer, "192.168.1.0/24", io::empty(), io::sink()).unwrap_err();
    assert!(unanswered.to_string().contains("did not answer"), "{unanswered}");
    hang_up.join().unwrap();
    // A helper killed outright leaves its socket file behind; the next session takes it.
    std::fs::remove_file(paths.control()).unwrap();
    drop(UnixListener::bind(paths.control()).unwrap());
    let mut next = Session::start(&paths, newer);
    assert_eq!(status(&paths).unwrap().proxy, Some(next.ready.proxy.to_string()));
    next.stop();
}

#[test]
fn of_two_sessions_starting_together_exactly_one_holds_the_bridge() {
    let (_root, paths) = paths();
    prepare(&paths).unwrap();
    let nonces = ["0123456789abcdef0123456789abcdef", "fedcba9876543210fedcba9876543210"];
    let mut starts = Vec::new();
    for nonce in nonces {
        owner_proxy(&paths.bridge(&Nonce::parse(nonce).unwrap()));
        let (input, heartbeat) = io::pipe().unwrap();
        let (output, ready) = io::pipe().unwrap();
        let paths = paths.clone();
        let helper = thread::spawn(move || hold::run(&paths, nonce, "192.168.1.0/24", input, ready));
        starts.push((helper, heartbeat, output));
    }
    let mut ready = Vec::new();
    let mut refused = 0;
    for (helper, heartbeat, output) in starts {
        if let Some(line) = read_line(&mut BufReader::new(output)).unwrap() {
            ready.push((helper, heartbeat, line));
        } else {
            let error = helper.join().unwrap().unwrap_err();
            assert!(error.to_string().contains("Another Horizon"), "{error}");
            refused += 1;
        }
    }
    assert_eq!((ready.len(), refused), (1, 1));
    let (helper, heartbeat, line) = ready.pop().unwrap();
    let proxy = serde_json::from_str::<Ready>(&line).unwrap().proxy;
    assert_eq!(status(&paths).unwrap().proxy, Some(proxy.to_string()));
    drop(heartbeat);
    helper.join().unwrap().unwrap();
}

#[test]
fn the_helper_refuses_invalid_sessions_and_a_missing_bridge_socket() {
    let (_root, paths) = paths();
    for (nonce, subnet) in [
        ("../../etc", "192.168.1.0/24"),
        (NONCE, "10.0.0.0/8"),
        (NONCE, "192.168.1.0/24; reboot"),
    ] {
        let result = hold::run(&paths, nonce, subnet, io::empty(), io::sink());
        assert!(result.is_err(), "{nonce} {subnet}");
    }
    let started = Instant::now();
    assert!(hold::run(&paths, NONCE, "192.168.1.0/24", io::empty(), io::sink()).is_err());
    assert!(started.elapsed() >= Duration::from_secs(9));
}

#[test]
fn preparing_creates_a_private_directory_and_refuses_other_files() {
    use std::os::unix::fs::PermissionsExt;
    let (root, paths) = paths();
    prepare(&paths).unwrap();
    prepare(&paths).unwrap();
    let mode = std::fs::metadata(&paths.directory).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    let file = Paths {
        directory: root.path().join("file"),
    };
    std::fs::write(&file.directory, "").unwrap();
    assert!(prepare(&file).is_err());
    assert_eq!(
        Paths::system().bridge(&Nonce::parse(NONCE).unwrap()).to_string_lossy(),
        Nonce::parse(NONCE).unwrap().bridge_socket()
    );
}

#[test]
fn requests_and_answers_keep_their_wire_shape() {
    let request = Request::Forward {
        host: "camera.local".into(),
        port: 554,
    };
    assert_eq!(
        serde_json::to_string(&request).unwrap(),
        r#"{"operation":"forward","host":"camera.local","port":554}"#
    );
    assert!(
        serde_json::from_str::<Request>(r#"{"operation":"forward","host":"x","port":1,"listen":"0.0.0.0"}"#).is_err()
    );
    assert!(serde_json::from_str::<Request>(r#"{"operation":"start"}"#).is_err());
    let off = serde_json::to_value(Status::off()).unwrap();
    assert_eq!(off["active"], false);
    assert!(off.get("proxy").is_none());
}

#[test]
fn probes_are_checked_here_and_answered_by_the_owner() {
    let (_root, paths) = paths();
    let mut session = Session::start(&paths, NONCE);
    // A Horizon that answers discovery but predates probes.
    session.tell(r#"{"hello":{"discovery":1,"sources":["mdns"]}}"#);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !status(&paths).unwrap().discovery.unwrap().available {
        assert!(Instant::now() < deadline, "no hello");
        thread::sleep(Duration::from_millis(20));
    }
    let refused = probe(&paths, "192.168.1.50", Vec::new()).unwrap_err().to_string();
    assert!(refused.contains("does not support probes"), "{refused}");

    session.tell(r#"{"hello":{"discovery":1,"sources":["mdns","probe"]}}"#);
    while !status(&paths).unwrap().discovery.unwrap().probe {
        assert!(Instant::now() < deadline, "no second hello");
        thread::sleep(Duration::from_millis(20));
    }
    // Requests the owner's Horizon would refuse fail here without asking it.
    for (host, ports) in [
        ("", vec![]),
        ("a b", vec![]),
        ("192.168.1.50", vec![0]),
        ("192.168.1.50", (1..=17).collect()),
    ] {
        assert!(probe(&paths, host, ports).is_err(), "{host}");
    }
    let asking = {
        let paths = paths.clone();
        thread::spawn(move || probe(&paths, "printer.local", vec![631, 80]))
    };
    let call = session.call();
    assert_eq!(
        call.request,
        discovery::Request::Probe {
            host: "printer.local".into(),
            ports: vec![631, 80]
        }
    );
    let result = discovery::Probe {
        host: "printer.local".into(),
        address: Ipv4Addr::new(192, 168, 1, 50),
        open: vec![631],
        closed: vec![80],
        silent: Vec::new(),
    };
    let answer = discovery::Message::Answer {
        id: call.id,
        answer: discovery::Answer::Probe(result.clone()),
    };
    session.tell(&serde_json::to_string(&answer).unwrap());
    assert_eq!(asking.join().unwrap().unwrap(), result);
    session.stop();
}
