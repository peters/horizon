use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command},
};

/// A user-level `sshd` on a free loopback port that accepts one generated key.
struct Server {
    child: Child,
    port: u16,
    root: tempfile::TempDir,
}

impl Server {
    fn start(sshd: &Path) -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        for key in ["host", "client"] {
            let status = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(path.join(key))
                .status()
                .unwrap();
            assert!(status.success());
        }
        std::fs::copy(path.join("client.pub"), path.join("authorized_keys")).unwrap();
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        std::fs::write(
            path.join("sshd_config"),
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {0}/host\nAuthorizedKeysFile {0}/authorized_keys\n\
                 PidFile {0}/sshd.pid\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n\
                 StrictModes no\nAllowTcpForwarding yes\nAllowStreamLocalForwarding yes\n",
                path.display()
            ),
        )
        .unwrap();
        let child = Command::new(sshd)
            .args(["-D", "-e", "-f"])
            .arg(path.join("sshd_config"))
            .arg("-E")
            .arg(path.join("sshd.log"))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "sshd did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        Self { child, port, root }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The production command shape against the fixture: its own user and host key, a
/// socket directory it can write, and a stand-in helper that confirms and reads heartbeats.
struct Fixture {
    port: u16,
    root: PathBuf,
    sockets: PathBuf,
}

impl Fixture {
    fn options(&self) -> Vec<String> {
        let user = String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout).unwrap();
        vec![
            "-F".into(),
            "none".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "StrictHostKeyChecking=accept-new".into(),
            "-o".into(),
            format!("UserKnownHostsFile={}", self.root.join("known_hosts").display()),
            "-o".into(),
            "GlobalKnownHostsFile=/dev/null".into(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
            "-i".into(),
            self.root.join("client").display().to_string(),
            "-p".into(),
            self.port.to_string(),
            format!("{}@127.0.0.1", user.trim()),
        ]
    }
}

impl session::Transport for Fixture {
    fn prepare(&self) -> Command {
        let mut command = Command::new("ssh");
        command
            .args(self.options())
            .arg("printf '%s\\n' horizon-local-network=1");
        command
    }

    fn hold(&self, nonce: &horizon_cloud_protocol::local_network::Nonce, _: Subnet, proxy_port: u16) -> Command {
        let mut command = Command::new("ssh");
        command
            .args(["-T", "-a", "-x", "-o", "ExitOnForwardFailure=yes", "-R"])
            .arg(format!(
                "{}/{nonce}.sock:127.0.0.1:{proxy_port}",
                self.sockets.display()
            ))
            .args(self.options())
            .arg(r#"printf '{"proxy":"127.0.0.1:41234"}\n'; exec cat > /dev/null"#);
        command
    }
}

/// Admits exactly one loopback fixture, which the real scope would refuse.
struct Only(SocketAddr);

impl socks::Gate for Only {
    fn admit(&self, destination: &Destination) -> Result<Vec<SocketAddr>, Reply> {
        match destination {
            Destination::Address(address) if *address == self.0 => Ok(vec![*address]),
            _ => Err(Reply::NotAllowed),
        }
    }
}

fn sshd() -> PathBuf {
    std::env::var_os("HORIZON_TEST_SSHD")
        .map(PathBuf::from)
        .expect("HORIZON_TEST_SSHD names an OpenSSH server binary")
}

/// Unix socket paths are limited to about 100 bytes, so the worker side lives directly in /tmp.
fn socket_directory() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("lnb").tempdir_in("/tmp").unwrap()
}

fn echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut socket in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut buffer = [0; 1024];
                while let Ok(count) = socket.read(&mut buffer) {
                    if count == 0 || socket.write_all(&buffer[..count]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    address
}

/// Opens a SOCKS5 CONNECT the way the worker helper does, over the forwarded socket.
fn connect(socket: &Path, destination: SocketAddr) -> std::io::Result<(UnixStream, u8)> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(&[5, 1, 0])?;
    let mut choice = [0; 2];
    stream.read_exact(&mut choice)?;
    let std::net::IpAddr::V4(ip) = destination.ip() else {
        panic!("IPv4 destination")
    };
    let mut request = vec![5, 1, 0, 1];
    request.extend(ip.octets());
    request.extend(destination.port().to_be_bytes());
    stream.write_all(&request)?;
    let mut reply = [0; 10];
    stream.read_exact(&mut reply)?;
    Ok((stream, reply[1]))
}

fn bridge_socket(directory: &Path) -> PathBuf {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let found = std::fs::read_dir(directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "sock"));
        if let Some(path) = found {
            return path;
        }
        assert!(Instant::now() < deadline, "sshd created no bridge socket");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "needs a local OpenSSH server named by HORIZON_TEST_SSHD"]
fn the_worker_socket_reaches_only_what_the_client_admits_and_closes_with_the_bridge() {
    let server = Server::start(&sshd());
    let sockets = socket_directory();
    let echo = echo_server();
    let refused = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let fixture = Fixture {
        port: server.port,
        root: server.root.path().to_owned(),
        sockets: sockets.path().to_owned(),
    };
    let proxy = Proxy::with_gate(subnet(), Arc::new(Only(echo))).unwrap();
    let bridge = Bridge::with_parts(proxy, fixture).unwrap();
    wait_for_state(&bridge, |state| matches!(state, State::Active { .. }));
    let socket = bridge_socket(sockets.path());
    let (mut stream, reply) = connect(&socket, echo).unwrap();
    assert_eq!(reply, 0);
    stream.write_all(b"frame").unwrap();
    let mut echoed = [0; 5];
    stream.read_exact(&mut echoed).unwrap();
    assert_eq!(&echoed, b"frame");
    let (_, reply) = connect(&socket, refused).unwrap();
    assert_eq!(reply, Reply::NotAllowed.code());
    let status = bridge.status();
    assert!(
        status.counters.bytes >= 10 && status.counters.refused == 1,
        "{status:?}"
    );
    let started = Instant::now();
    drop(bridge);
    assert!(started.elapsed() < Duration::from_secs(10));
    let mut byte = [0; 1];
    assert!(matches!(stream.read(&mut byte), Ok(0) | Err(_)));
    let deadline = Instant::now() + Duration::from_secs(10);
    while connect(&socket, echo).is_ok_and(|(_, reply)| reply == 0) {
        assert!(Instant::now() < deadline, "the worker socket outlived the bridge");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "needs a local OpenSSH server named by HORIZON_TEST_SSHD"]
fn a_forward_the_worker_cannot_create_ends_the_session_with_the_ssh_error() {
    let server = Server::start(&sshd());
    let sockets = socket_directory();
    let fixture = Fixture {
        port: server.port,
        root: server.root.path().to_owned(),
        sockets: sockets.path().join("missing"),
    };
    let proxy = Proxy::with_gate(subnet(), Arc::new(Only(echo_server()))).unwrap();
    let bridge = Bridge::with_parts(proxy, fixture).unwrap();
    let State::Reconnecting { error } = wait_for_state(&bridge, |state| matches!(state, State::Reconnecting { .. }))
    else {
        unreachable!()
    };
    assert!(error.contains("forwarding failed"), "{error}");
}
