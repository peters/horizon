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
        let line = read_line(&mut BufReader::new(output)).unwrap().unwrap();
        Self {
            heartbeat: Some(heartbeat),
            ready: serde_json::from_str(&line).unwrap(),
            helper: Some(helper),
        }
    }

    fn stop(&mut self) {
        drop(self.heartbeat.take());
        self.helper.take().unwrap().join().unwrap().unwrap();
    }
}

use horizon_cloud_protocol::local_network::Ready;

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
